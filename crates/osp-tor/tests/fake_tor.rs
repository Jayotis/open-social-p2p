//! The node's Tor supervisor against a fake Tor control port: it
//! authenticates, sets up the onion service, and does it all again when
//! Tor goes away and comes back.

use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::Result;
use data_encoding::HEXUPPER;
use iroh::{EndpointId, SecretKey};
use osp_tor::{Phase, Source, Tor, TorState, TorTransport, onion};
use ring::hmac;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    sync::watch,
    task::AbortHandle,
    time::timeout,
};

/// Answers the commands the node sends, as Tor 0.4.9 does.
struct FakeTor {
    cookie: [u8; 32],
    cookie_file: PathBuf,
    /// Its own cookie is wrong: it isn't the real Tor.
    impostor: bool,
    /// The onion service it names in reply to `ADD_ONION`.
    service_id: String,
    commands: Mutex<Vec<String>>,
    connections: Mutex<Vec<AbortHandle>>,
}

impl FakeTor {
    /// A fake Tor that names the onion services it sets up as `names`'s.
    async fn start(
        dir: &tempfile::TempDir,
        names: &EndpointId,
        impostor: bool,
    ) -> (Arc<Self>, SocketAddr) {
        let cookie = [5; 32];
        let cookie_file = dir.path().join("control_auth_cookie");
        std::fs::write(&cookie_file, cookie).unwrap();
        let fake = Arc::new(Self {
            cookie,
            cookie_file,
            impostor,
            service_id: onion::service_id(names),
            commands: Mutex::default(),
            connections: Mutex::default(),
        });
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let serving = fake.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let task = tokio::spawn(serving.clone().serve(stream));
                serving
                    .connections
                    .lock()
                    .unwrap()
                    .push(task.abort_handle());
            }
        });
        (fake, addr)
    }

    /// Closes every connection, as Tor stopping does.
    fn stop(&self) {
        for conn in self.connections.lock().unwrap().drain(..) {
            conn.abort();
        }
    }

    /// The commands received, by their first word.
    fn count(&self, verb: &str) -> usize {
        let commands = self.commands.lock().unwrap();
        commands
            .iter()
            .filter(|c| c.split(' ').next() == Some(verb))
            .count()
    }

    async fn serve(self: Arc<Self>, stream: TcpStream) {
        let (read, mut write) = stream.into_split();
        let mut lines = BufReader::new(read).lines();
        let mut client_hash = None;
        while let Ok(Some(line)) = lines.next_line().await {
            self.commands.lock().unwrap().push(line.clone());
            let mut words = line.split(' ');
            let reply = match (words.next().unwrap(), words.next()) {
                ("PROTOCOLINFO", _) => format!(
                    "250-PROTOCOLINFO 1\r\n\
                     250-AUTH METHODS=COOKIE,SAFECOOKIE COOKIEFILE=\"{}\"\r\n\
                     250-VERSION Tor=\"0.4.9.15\"\r\n250 OK\r\n",
                    self.cookie_file.display()
                ),
                ("AUTHCHALLENGE", Some("SAFECOOKIE")) => {
                    let client_nonce = HEXUPPER.decode(words.next().unwrap().as_bytes()).unwrap();
                    let server_nonce = [9; 32];
                    let mut msg = self.cookie.to_vec();
                    if self.impostor {
                        msg = vec![0; 32];
                    }
                    msg.extend_from_slice(&client_nonce);
                    msg.extend_from_slice(&server_nonce);
                    let key = |k: &[u8]| hmac::Key::new(hmac::HMAC_SHA256, k);
                    let server = hmac::sign(
                        &key(b"Tor safe cookie authentication server-to-controller hash"),
                        &msg,
                    );
                    let client = hmac::sign(
                        &key(b"Tor safe cookie authentication controller-to-server hash"),
                        &msg,
                    );
                    client_hash = Some(HEXUPPER.encode(client.as_ref()));
                    format!(
                        "250 AUTHCHALLENGE SERVERHASH={} SERVERNONCE={}\r\n",
                        HEXUPPER.encode(server.as_ref()),
                        HEXUPPER.encode(&server_nonce)
                    )
                }
                ("AUTHENTICATE", hash) if hash.map(str::to_string) == client_hash => {
                    "250 OK\r\n".to_string()
                }
                ("AUTHENTICATE", _) => "515 Authentication failed\r\n".to_string(),
                ("SETEVENTS", _) | ("DEL_ONION", _) => "250 OK\r\n".to_string(),
                ("GETINFO", Some("net/listeners/socks")) => {
                    "250-net/listeners/socks=\"127.0.0.1:9\"\r\n250 OK\r\n".to_string()
                }
                ("GETINFO", Some("status/bootstrap-phase")) => {
                    "250-status/bootstrap-phase=NOTICE BOOTSTRAP PROGRESS=100 TAG=done SUMMARY=\"Done\"\r\n250 OK\r\n"
                        .to_string()
                }
                ("ADD_ONION", _) => format!(
                    "250-ServiceID={id}\r\n250 OK\r\n\
                     650 HS_DESC UPLOADED {id} UNKNOWN $AAAA\r\n",
                    id = self.service_id
                ),
                _ => "510 Unrecognized command\r\n".to_string(),
            };
            if write.write_all(reply.as_bytes()).await.is_err() {
                break;
            }
        }
    }
}

/// Waits until Tor's state passes `check`.
async fn wait_for(
    state: &mut watch::Receiver<TorState>,
    check: impl Fn(&TorState) -> bool,
) -> TorState {
    let waited = timeout(Duration::from_secs(10), async {
        state.wait_for(|s| check(s)).await.map(|s| s.clone())
    })
    .await;
    match waited {
        Ok(Ok(s)) => s,
        _ => panic!("Tor's state stayed {:?}", *state.borrow()),
    }
}

#[tokio::test]
async fn sets_up_the_onion_service_again_after_tor_restarts() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let key = SecretKey::generate();
    let (fake, addr) = FakeTor::start(&dir, &key.public(), false).await;
    let tor = Tor::start(Source::External(addr));
    let transport = TorTransport::new(&tor, key).await?;
    let mut state = tor.state();
    let s = wait_for(&mut state, |s| s.phase == Phase::Ready && s.published == 1).await;
    assert_eq!(s.socks, Some("127.0.0.1:9".parse()?));
    assert_eq!(s.onion_services, 1);
    assert_eq!(fake.count("ADD_ONION"), 1);

    fake.stop();
    let s = wait_for(&mut state, |s| matches!(s.phase, Phase::Down(_))).await;
    assert_eq!((s.restarts, s.published, s.socks), (1, 0, None));
    // Back after a pause, with the onion service set up again.
    wait_for(&mut state, |s| s.phase == Phase::Ready && s.published == 1).await;
    assert_eq!(fake.count("ADD_ONION"), 2);

    // Gone with the transport.
    drop(transport);
    wait_for(&mut state, |s| s.onion_services == 0).await;
    assert_eq!(fake.count("DEL_ONION"), 1);
    Ok(())
}

#[tokio::test]
async fn refuses_a_control_port_that_doesnt_know_tors_cookie() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let key = SecretKey::generate();
    let (fake, addr) = FakeTor::start(&dir, &key.public(), true).await;
    let tor = Tor::start(Source::External(addr));
    let s = wait_for(&mut tor.state(), |s| matches!(s.phase, Phase::Down(_))).await;
    let Phase::Down(why) = s.phase else {
        unreachable!()
    };
    assert!(why.contains("doesn't know Tor's cookie"), "{why}");
    assert_eq!(
        fake.count("AUTHENTICATE"),
        0,
        "never answered the challenge"
    );
    Ok(())
}

#[tokio::test]
async fn says_when_tor_names_the_onion_service_otherwise() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let other = SecretKey::generate().public();
    let (_fake, addr) = FakeTor::start(&dir, &other, false).await;
    let tor = Tor::start(Source::External(addr));
    let _transport = TorTransport::new(&tor, SecretKey::generate()).await?;
    let s = wait_for(&mut tor.state(), |s| s.onion_error.is_some()).await;
    let error = s.onion_error.unwrap();
    assert!(error.contains("as the node's key gives"), "{error}");
    Ok(())
}

#[tokio::test]
async fn says_when_tor_isnt_installed() {
    let dir = tempfile::tempdir().unwrap();
    let tor = Tor::start(Source::Launch {
        binary: "/nonexistent/tor".into(),
        dir: dir.path().join("tor"),
    });
    let s = wait_for(&mut tor.state(), |s| matches!(s.phase, Phase::Down(_))).await;
    assert!(
        s.phase.to_string().contains("isn't installed"),
        "{}",
        s.phase
    );
}
