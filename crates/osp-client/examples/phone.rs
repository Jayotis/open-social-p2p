//! A pretend phone for testing a home node from a computer, using the same
//! client code as the Android app. Run it on another machine (or network)
//! to test the path a phone away from home would take.
//!
//! ```sh
//! cargo run -p osp-client --example phone -- pair <ticket from `osp pair`>
//! cargo run -p osp-client --example phone -- status
//! cargo run -p osp-client --example phone -- feed
//! cargo run -p osp-client --example phone -- post "Hello from the phone" clip.mp4
//! cargo run -p osp-client --example phone -- get <blob hash> out.mp4
//! ```
//!
//! State is kept in `--dir` (default `./phone-sim`).

use std::{path::PathBuf, time::Instant};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use osp_client::{Client, ClientOptions};
use osp_proto::{
    BlobHash,
    rpc::{ClientRequest, ClientResponse},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Parser)]
struct Cli {
    /// Where the pretend phone keeps its key and pairing
    #[arg(long, default_value = "phone-sim")]
    dir: PathBuf,
    /// Bind to this address only. `127.0.0.1:0` leaves the relay as the
    /// only way to the node, like a phone away from home.
    #[arg(long)]
    bind: Option<std::net::SocketAddr>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Pair with a home node
    Pair {
        ticket: String,
        #[arg(long, default_value = "Pretend phone")]
        name: String,
    },
    /// Show the node's status and how this device is connected
    Status,
    /// Show the feed
    Feed,
    /// Post text with optional files
    Post { text: String, files: Vec<PathBuf> },
    /// Download a blob through the local media server, as the player would
    Get { blob: BlobHash, out: PathBuf },
    /// Forget the pairing
    Forget,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "osp_client=debug,iroh=warn".into()),
        )
        .init();
    let cli = Cli::parse();
    let options = ClientOptions {
        bind: cli.bind,
        ..ClientOptions::default()
    };
    let client = Client::open(&cli.dir, options).await?;
    let result = run(&client, cli.command).await;
    client.shutdown().await;
    result
}

async fn run(client: &std::sync::Arc<Client>, command: Command) -> Result<()> {
    match command {
        Command::Pair { ticket, name } => {
            let started = Instant::now();
            let p = client.pair(&ticket, &name).await?;
            println!(
                "Paired with {}'s node in {:.1} s",
                p.name,
                started.elapsed().as_secs_f64()
            );
            println!("This device: {}", client.device_id());
        }
        Command::Status => {
            let started = Instant::now();
            let ClientResponse::Status(s) = client.request(&ClientRequest::Status).await? else {
                bail!("unexpected reply");
            };
            println!(
                "{} · node {} · version {} · up {} s · reply in {} ms",
                s.name,
                s.node.fmt_short(),
                s.version,
                s.uptime_secs,
                started.elapsed().as_millis()
            );
            for problem in &s.problems {
                println!("Problem: {problem}");
            }
            if let Some(info) = client.connection_info().await {
                let rtt = info.rtt_ms.map(|r| format!(", {r} ms")).unwrap_or_default();
                println!("Connected: {}{rtt}", info.path);
            }
        }
        Command::Feed => {
            for item in client.feed(20).await? {
                let author = item
                    .actor_name
                    .unwrap_or_else(|| item.actor.fmt_short().to_string());
                println!("{author} · {}  {}", item.published, item.content);
                for a in item.attachments {
                    println!(
                        "    {} {} ({} bytes)",
                        a.attachment.media_type, a.attachment.blob, a.attachment.size
                    );
                }
            }
        }
        Command::Post { text, files } => {
            let started = Instant::now();
            let id = client
                .post(text, &files, |done, total| {
                    eprint!("\rsent {} of {} KB", done / 1024, total / 1024);
                })
                .await?;
            eprintln!();
            println!("Published {id} in {:.1} s", started.elapsed().as_secs_f64());
        }
        Command::Get { blob, out } => {
            let started = Instant::now();
            let url = client.media_url(&blob, "video/mp4").await?;
            let bytes = http_get(&url).await?;
            std::fs::write(&out, &bytes).with_context(|| format!("writing {}", out.display()))?;
            let secs = started.elapsed().as_secs_f64();
            println!(
                "{} bytes in {secs:.1} s ({:.1} Mbit/s), BLAKE3 {}",
                bytes.len(),
                bytes.len() as f64 * 8.0 / secs / 1e6,
                if blake3::hash(&bytes).as_bytes() == blob.as_bytes() {
                    "matches"
                } else {
                    "DOES NOT MATCH"
                }
            );
        }
        Command::Forget => {
            client.forget().await?;
            println!("Forgot the pairing.");
        }
    }
    Ok(())
}

async fn http_get(url: &str) -> Result<Vec<u8>> {
    let rest = url.strip_prefix("http://").context("not an http URL")?;
    let (host, path) = rest.split_once('/').context("no path")?;
    let mut stream = tokio::net::TcpStream::connect(host).await?;
    stream
        .write_all(format!("GET /{path} HTTP/1.1\r\nHost: {host}\r\n\r\n").as_bytes())
        .await?;
    let mut resp = Vec::new();
    stream.read_to_end(&mut resp).await?;
    let split = resp
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .context("no response head")?;
    let status = String::from_utf8_lossy(&resp[..split]);
    if !status.starts_with("HTTP/1.1 200") {
        bail!("{}", status.lines().next().unwrap_or_default());
    }
    Ok(resp[split + 4..].to_vec())
}
