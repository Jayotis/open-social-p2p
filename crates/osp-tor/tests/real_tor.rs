//! Two nodes on this machine, each with its own Tor, talking through the
//! Tor network, while each Tor in turn is killed and started again by its
//! node. Needs `tor` and the internet, and takes minutes, so it only runs
//! when asked:
//!
//! ```text
//! OSP_TOR=/usr/bin/tor cargo test -p osp-tor --test real_tor -- --ignored --nocapture
//! ```

use std::{path::Path, time::Duration};

use anyhow::{Context, Result, bail};
use iroh::{
    Endpoint, EndpointId, SecretKey,
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler, Router},
};
use osp_tor::{Phase, Source, Tor, TorTransport, onion};
use tokio::time::{Instant, sleep, timeout};

const ALPN: &[u8] = b"osp-tor-test/0";

#[derive(Debug, Clone)]
struct Echo;

impl ProtocolHandler for Echo {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        while let Ok((mut send, mut recv)) = conn.accept_bi().await {
            tokio::spawn(async move {
                if let Ok(data) = recv.read_to_end(1 << 20).await {
                    let _ = send.write_all(&data).await;
                    let _ = send.finish();
                }
            });
        }
        Ok(())
    }
}

async fn echo(conn: &Connection) -> Result<Duration> {
    let started = Instant::now();
    let (mut send, mut recv) = conn.open_bi().await?;
    send.write_all(b"hello").await?;
    send.finish()?;
    let back = recv.read_to_end(1024).await?;
    anyhow::ensure!(back == b"hello", "echoed something else");
    Ok(started.elapsed())
}

/// Connects and echoes, trying again until it works or `limit` passes.
/// Returns the connection and how long it took.
async fn reach(
    ep: &Endpoint,
    to: EndpointId,
    conn: Option<Connection>,
    limit: Duration,
) -> Result<(Connection, Duration)> {
    let started = Instant::now();
    let mut conn = conn;
    while started.elapsed() < limit {
        let c = match conn.take() {
            Some(c) => c,
            None => match timeout(Duration::from_secs(60), ep.connect(to, ALPN)).await {
                Ok(Ok(c)) => c,
                Ok(Err(e)) => {
                    println!(
                        "  connect failed after {:.1}s: {e:#}",
                        started.elapsed().as_secs_f64()
                    );
                    sleep(Duration::from_secs(2)).await;
                    continue;
                }
                Err(_) => {
                    println!(
                        "  connect timed out after {:.1}s",
                        started.elapsed().as_secs_f64()
                    );
                    continue;
                }
            },
        };
        match timeout(Duration::from_secs(20), echo(&c)).await {
            Ok(Ok(_)) => return Ok((c, started.elapsed())),
            Ok(Err(e)) => println!(
                "  echo failed after {:.1}s: {e:#}",
                started.elapsed().as_secs_f64()
            ),
            Err(_) => println!(
                "  echo timed out after {:.1}s",
                started.elapsed().as_secs_f64()
            ),
        }
    }
    bail!("not reached within {}s", limit.as_secs())
}

async fn node(tor: &Tor, key: SecretKey) -> Result<(Endpoint, TorTransport)> {
    let transport = TorTransport::new(tor, key.clone()).await?;
    let ep = Endpoint::builder(transport.preset())
        .secret_key(key)
        .bind()
        .await?;
    Ok((ep, transport))
}

/// Kills the Tor started in `dir`, as a crash would.
fn kill_tor(dir: &Path) -> Result<()> {
    let pattern = dir.join("torrc");
    let status = std::process::Command::new("pkill")
        .arg("-f")
        .arg(&pattern)
        .status()?;
    anyhow::ensure!(
        status.success(),
        "no Tor running from {}",
        pattern.display()
    );
    Ok(())
}

async fn wait_published(tor: &Tor, name: &str) -> Result<Duration> {
    let started = Instant::now();
    let mut state = tor.state();
    timeout(
        Duration::from_secs(300),
        state.wait_for(|s| s.phase == Phase::Ready && s.published >= 1),
    )
    .await
    .with_context(|| format!("{name}'s onion service wasn't published"))??;
    Ok(started.elapsed())
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs tor and the Tor network; takes minutes"]
async fn nodes_talk_through_tor_and_survive_its_restarts() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "osp_tor=info".into()),
        )
        .try_init()
        .ok();
    let binary = std::env::var("OSP_TOR").unwrap_or_else(|_| "tor".into());
    let dir = tempfile::tempdir()?;
    let (dir_a, dir_b) = (dir.path().join("a"), dir.path().join("b"));
    let launch = |dir: &Path| {
        Tor::start(Source::Launch {
            binary: binary.clone().into(),
            dir: dir.to_path_buf(),
        })
    };
    let (tor_a, tor_b) = (launch(&dir_a), launch(&dir_b));
    // A fixed key, so Tor's name for its onion service checks ours.
    let key_b = SecretKey::from_bytes(&[7; 32]);
    println!("b's onion service: {}", onion::service_id(&key_b.public()));
    let (a, _) = node(&tor_a, SecretKey::generate()).await?;
    let (b, b_transport) = node(&tor_b, key_b).await?;
    let _router = Router::builder(b.clone()).accept(ALPN, Echo).spawn();

    let took = wait_published(&tor_b, "b").await?;
    println!("b published in {:.1}s", took.as_secs_f64());
    wait_published(&tor_a, "a").await?;

    let (conn, took) = reach(&a, b.id(), None, Duration::from_secs(300)).await?;
    println!("first connection and echo: {:.1}s", took.as_secs_f64());
    let mut rtts = Vec::new();
    for _ in 0..10 {
        rtts.push(echo(&conn).await?.as_secs_f64());
    }
    rtts.sort_by(f64::total_cmp);
    println!(
        "echo round trips: median {:.2}s, max {:.2}s",
        rtts[5], rtts[9]
    );
    println!("b's streams: {:?}", b_transport.streams());

    println!("killing b's Tor");
    kill_tor(&dir_b)?;
    let started = Instant::now();
    let (conn, _) = reach(&a, b.id(), Some(conn), Duration::from_secs(600)).await?;
    println!(
        "after b's Tor restarted: echo again in {:.1}s",
        started.elapsed().as_secs_f64()
    );

    println!("killing a's Tor");
    kill_tor(&dir_a)?;
    let started = Instant::now();
    reach(&a, b.id(), Some(conn), Duration::from_secs(600)).await?;
    println!(
        "after a's Tor restarted: echo again in {:.1}s",
        started.elapsed().as_secs_f64()
    );
    for (name, tor) in [("a", &tor_a), ("b", &tor_b)] {
        let s = tor.state().borrow().clone();
        println!("{name}'s Tor: {}, {} restart(s)", s.phase, s.restarts);
        anyhow::ensure!(
            s.restarts == 1,
            "{name}'s Tor restarted {} times",
            s.restarts
        );
    }
    Ok(())
}
