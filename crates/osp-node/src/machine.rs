//! The node's machine name (ADR 0015): a second identity on Tor, tied to no
//! person, under which the node gets to know other nodes.
//!
//! Two rules keep it apart from its owner. What a machine says comes only from
//! what the machine itself learned, never from the owner's follows, posts,
//! media, trusted followers or peer records, so nothing here touches those.
//! And nothing sent under the public ID mentions the machine name: it isn't in
//! `Status`, which goes to phones and gets pasted, and its ID is never logged,
//! as a log holds the public ID too. Only the local `osp machine` shows it.
//!
//! That isn't anonymity: both keys share one Tor and come and go together.

use std::{
    fmt,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use iroh::{
    Endpoint, EndpointId, SecretKey,
    endpoint::{Connection, RecvStream, SendStream},
    protocol::{AcceptError, ProtocolHandler},
};
use osp_proto::{
    MACHINE_ALPN,
    framing::{read_frame_max, write_frame},
    machine::{
        MAX_REPLY_LEN, MAX_REQUEST_LEN, MachineInfo, MachineRecord, MachineRequest, MachineResponse,
    },
    rpc::ClientResponse,
};
use osp_tor::TorTransport;
use tokio::sync::Semaphore;
use tracing::{debug, info};

use crate::{
    config::Config,
    home::Home,
    keys,
    node::{Core, SimSettings},
    social,
    tor::TorSide,
};

/// Machines served at once. A conversation is short, and being busy costs a
/// machine nothing: it tries again another time.
const MAX_CONNECTIONS: usize = 16;
/// Requests at once on one connection.
const MAX_STREAMS: u32 = 4;
/// How long one machine's connection is served.
const CONNECTION_LIMIT: Duration = Duration::from_secs(60);
/// Give up on one request after this long.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Longest a greeting waits to connect: a first connection over Tor takes up
/// to a minute.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(90);

/// The QUIC error code when a machine is told to come back later, or a
/// conversation is over.
const CLOSE_BUSY: u32 = 1;
const CLOSE_DONE: u32 = 0;

pub(crate) struct MachineSide {
    key: SecretKey,
    pub endpoint: Endpoint,
    /// Holds the onion service up, on Tor.
    _transport: Option<TorTransport>,
}

impl MachineSide {
    /// Starts the node's machine name, making its key if it's new: on Tor, or
    /// on the simulated network if that's asked for. Without either, or with
    /// `[machine]` off, the node has none.
    pub async fn start(
        home: &Home,
        config: &Config,
        tor: Option<&TorSide>,
        #[cfg_attr(not(feature = "sim"), allow(unused_variables))] sim: &SimSettings,
    ) -> Result<Option<Self>> {
        if !config.machine.enabled {
            return Ok(None);
        }
        let path = home.machine_key();
        let load = || match path.exists() {
            true => keys::load(&path),
            false => keys::create(&path),
        };
        let side = if let Some(tor) = tor {
            let key = load()?;
            let (transport, endpoint) = tor.endpoint_for(key.clone(), MAX_STREAMS).await?;
            Some(Self {
                key,
                endpoint,
                _transport: Some(transport),
            })
        } else {
            #[cfg(feature = "sim")]
            match (&sim.network, sim.machine_names) {
                (Some(network), true) => {
                    let key = load()?;
                    let endpoint = Endpoint::builder(iroh::endpoint::presets::Minimal)
                        .relay_mode(iroh::RelayMode::Disabled)
                        .clear_ip_transports()
                        .add_custom_transport(network.transport(key.public()))
                        .address_lookup(network.lookup())
                        .secret_key(key.clone())
                        .bind()
                        .await
                        .context("starting the machine endpoint")?;
                    Some(Self {
                        key,
                        endpoint,
                        _transport: None,
                    })
                }
                _ => None,
            }
            #[cfg(not(feature = "sim"))]
            None
        };
        if side.is_some() {
            info!("the machine name is on");
        }
        Ok(side)
    }

    pub fn id(&self) -> EndpointId {
        self.key.public()
    }

    pub fn protocol(&self) -> MachineProtocol {
        MachineProtocol {
            key: self.key.clone(),
            connections: Arc::new(Semaphore::new(MAX_CONNECTIONS)),
        }
    }

    /// Greets another machine, and closes the connection: a conversation is
    /// an event, not something kept open. Returns what it says of itself, and
    /// how long connecting and asking took.
    async fn hello(&self, machine: EndpointId) -> Result<(MachineInfo, Duration, Duration)> {
        let started = Instant::now();
        let conn = tokio::time::timeout(
            CONNECT_TIMEOUT,
            self.endpoint.connect(machine, MACHINE_ALPN),
        )
        .await
        .with_context(|| format!("reaching machine {} timed out", words(&machine)))?
        .with_context(|| format!("reaching machine {}", words(&machine)))?;
        let connected = started.elapsed();
        let started = Instant::now();
        let reply = ask_hello(&conn, &self.key).await;
        conn.close(CLOSE_DONE.into(), b"done");
        Ok((checked(&reply?, machine)?, connected, started.elapsed()))
    }
}

async fn ask_hello(conn: &Connection, key: &SecretKey) -> Result<MachineRecord> {
    let (mut send, mut recv) = conn.open_bi().await?;
    let request = MachineRequest::Hello {
        record: own_record(key)?,
    };
    write_frame(&mut send, &request).await?;
    send.finish()?;
    let reply = tokio::time::timeout(REQUEST_TIMEOUT, read_frame_max(&mut recv, MAX_REPLY_LEN))
        .await
        .context("no reply in time")??;
    match reply {
        MachineResponse::Hello { record } => Ok(record),
        MachineResponse::Error { message } => anyhow::bail!("it answered: {message}"),
    }
}

/// This machine's record, freshly dated. It offers nothing yet.
fn own_record(key: &SecretKey) -> Result<MachineRecord> {
    MachineRecord::new(key, Vec::new(), osp_proto::now_rfc3339())
}

/// Checks the record of the machine at the other end of a connection.
fn checked(record: &MachineRecord, from: EndpointId) -> Result<MachineInfo> {
    let info = record.verify()?;
    ensure!(
        info.machine == from,
        "that's another machine's record, not the record of the machine connected"
    );
    social::dated(&info.published, "machine record")?;
    Ok(info)
}

/// Accepts incoming `osp/machine/1` connections.
#[derive(Clone)]
pub(crate) struct MachineProtocol {
    key: SecretKey,
    connections: Arc<Semaphore>,
}

impl fmt::Debug for MachineProtocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("MachineProtocol")
    }
}

impl ProtocolHandler for MachineProtocol {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        let Ok(_place) = self.connections.clone().try_acquire_owned() else {
            conn.close(CLOSE_BUSY.into(), b"busy");
            return Ok(());
        };
        // Anyone can make a name for free, so one gets a short, bounded turn.
        let _ = tokio::time::timeout(CONNECTION_LIMIT, async {
            let remote = conn.remote_id();
            while let Ok((send, recv)) = conn.accept_bi().await {
                let served = tokio::time::timeout(
                    REQUEST_TIMEOUT,
                    serve_stream(&self.key, remote, send, recv),
                )
                .await
                .unwrap_or_else(|_| Err(anyhow::anyhow!("took too long")));
                if let Err(e) = served {
                    debug!(remote = %remote.fmt_short(), "machine request failed: {e:#}");
                }
            }
        })
        .await;
        Ok(())
    }
}

async fn serve_stream(
    key: &SecretKey,
    remote: EndpointId,
    mut send: SendStream,
    mut recv: RecvStream,
) -> Result<()> {
    let request: MachineRequest = read_frame_max(&mut recv, MAX_REQUEST_LEN).await?;
    let reply = match request {
        MachineRequest::Hello { record } => match checked(&record, remote) {
            Ok(_) => MachineResponse::Hello {
                record: own_record(key)?,
            },
            Err(e) => MachineResponse::Error {
                message: format!("{e:#}"),
            },
        },
        MachineRequest::Unknown => MachineResponse::Error {
            message: "this machine doesn't know that request".to_string(),
        },
    };
    write_frame(&mut send, &reply).await?;
    send.finish()?;
    Ok(())
}

impl Core {
    fn machine(&self) -> Result<&MachineSide> {
        self.machine.as_ref().context(
            "this node has no machine name: it needs enabled = true under [tor] and under [machine] in config.toml",
        )
    }

    /// This node's machine name, for its owner.
    pub(crate) fn machine_name(&self) -> Result<ClientResponse> {
        let machine = self.machine()?.id();
        Ok(ClientResponse::MachineName {
            words: words(&machine),
            machine,
        })
    }

    /// Greets another machine, for `osp machine hello`.
    pub(crate) async fn machine_hello(&self, machine: EndpointId) -> Result<ClientResponse> {
        let side = self.machine()?;
        if let Some(tor) = &self.tor {
            tor.check_ready()?;
        }
        let (info, connected, asked) = side.hello(machine).await?;
        Ok(ClientResponse::MachineGreeted {
            words: words(&machine),
            offers: info.offers,
            connect_ms: connected.as_millis() as u64,
            round_trip_ms: asked.as_millis() as u64,
        })
    }
}

/// Three words made from a machine's ID, to tell machines apart by eye. Its
/// ID is what counts: anyone could make a key with the same words. The owner
/// never picks them, as a chosen name could give them away.
pub(crate) fn words(machine: &EndpointId) -> String {
    let list = bip39::Language::English.word_list();
    let b = machine.as_bytes();
    let bits = u64::from_be_bytes([0, 0, 0, b[0], b[1], b[2], b[3], b[4]]) >> 7;
    [bits >> 22, bits >> 11, bits]
        .map(|i| list[(i & 2047) as usize])
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_machine_is_shown_as_three_words_that_stay_the_same() {
        let key = SecretKey::from_bytes(&[7; 32]);
        let shown = words(&key.public());
        assert_eq!(shown, words(&key.public()));
        assert_eq!(shown.split('-').count(), 3);
        let list = bip39::Language::English.word_list();
        assert!(shown.split('-').all(|word| list.contains(&word)), "{shown}");
        assert_ne!(shown, words(&SecretKey::from_bytes(&[8; 32]).public()));
    }

    #[test]
    fn a_record_has_to_be_the_machine_connecteds_and_sensibly_dated() {
        let key = SecretKey::from_bytes(&[7; 32]);
        let me = key.public();
        checked(&own_record(&key).unwrap(), me).unwrap();

        let other = SecretKey::from_bytes(&[8; 32]).public();
        let why = checked(&own_record(&key).unwrap(), other).unwrap_err();
        assert!(why.to_string().contains("another machine"), "{why:#}");

        let ahead = humantime::format_rfc3339_seconds(
            std::time::SystemTime::now() + Duration::from_secs(30 * 24 * 60 * 60),
        );
        let far = MachineRecord::new(&key, vec![], ahead.to_string()).unwrap();
        let why = checked(&far, me).unwrap_err();
        assert!(why.to_string().contains("ahead"), "{why:#}");

        let undated = MachineRecord::new(&key, vec![], "soon".into()).unwrap();
        checked(&undated, me).unwrap_err();
    }
}
