//! The node's own Tor: started from the system's `tor` program in a folder
//! of the node's, watched, and started again if it stops, with the node's
//! onion services set up again each time (ADR 0014). A home node runs
//! unattended, so one that stopped talking until someone restarted it
//! wouldn't do.

use std::{
    collections::{BTreeMap, VecDeque},
    fmt,
    net::SocketAddr,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering::Relaxed},
    },
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use iroh::SecretKey;
use n0_future::task::AbortOnDropHandle;
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, BufReader},
    process::{Child, Command},
    sync::{mpsc, watch},
    time::Instant,
};
use tracing::{debug, info, warn};

use crate::{
    control::{Control, Reply},
    onion,
};

/// Longest Tor may take to open its control port after starting.
const START_TIMEOUT: Duration = Duration::from_secs(30);
/// How often to ask Tor how far it has got, while it connects, and once
/// it's ready.
const CHECK_CONNECTING: Duration = Duration::from_secs(2);
const CHECK_READY: Duration = Duration::from_secs(60);
/// The pause before starting Tor again doubles each time it stops soon
/// after starting, from the first to the second.
const PAUSES: (Duration, Duration) = (Duration::from_secs(2), Duration::from_secs(60));
/// A Tor that ran this long before stopping is started again after the
/// shortest pause.
const STABLE: Duration = Duration::from_secs(300);
/// Tor's last lines kept, to say why it didn't start.
const LAST_LINES: usize = 10;

/// Where the node's Tor comes from.
#[derive(Debug, Clone)]
pub enum Source {
    /// Start `binary` with a folder of its own, `dir`: its settings, its
    /// data and its cookie. It ends when the node does.
    Launch { binary: PathBuf, dir: PathBuf },
    /// A Tor that's already running, whose control port is at this address
    /// and takes cookie authentication. For tests, and for trying things.
    External(SocketAddr),
}

/// How the node's Tor is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    Starting,
    /// Connecting to the Tor network, this far from 0 to 100.
    Connecting(u8),
    Ready,
    /// Stopped, and why. It's started again after a pause.
    Down(String),
}

impl fmt::Display for Phase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Phase::Starting => write!(f, "starting"),
            Phase::Connecting(p) => write!(f, "connecting to the Tor network ({p}%)"),
            Phase::Ready => write!(f, "ready"),
            Phase::Down(why) => write!(f, "down: {why}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TorState {
    pub phase: Phase,
    /// Tor's SOCKS port, while Tor runs.
    pub socks: Option<SocketAddr>,
    /// How many times Tor has stopped since the node started.
    pub restarts: u32,
    /// The node's onion services, and how many of them Tor has published
    /// since it last started: until then, nobody can reach them.
    pub onion_services: usize,
    pub published: usize,
    /// Why Tor refused to set up an onion service, the last time it did.
    pub onion_error: Option<String>,
}

/// The node's Tor. Clones share it; it stops when the last is dropped.
#[derive(Clone)]
pub struct Tor {
    state: watch::Receiver<TorState>,
    requests: mpsc::UnboundedSender<Request>,
    next_service: Arc<AtomicU64>,
    _task: Arc<AbortOnDropHandle<()>>,
}

impl fmt::Debug for Tor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tor")
            .field("state", &*self.state.borrow())
            .finish()
    }
}

enum Request {
    Add { id: u64, service: Box<Service> },
    Remove { id: u64 },
}

struct Service {
    key: SecretKey,
    port: u16,
    target: SocketAddr,
    published: bool,
}

/// An onion service the node's Tor keeps up, through restarts, until this
/// is dropped.
pub(crate) struct OnionService {
    id: u64,
    requests: mpsc::UnboundedSender<Request>,
}

impl Drop for OnionService {
    fn drop(&mut self) {
        let _ = self.requests.send(Request::Remove { id: self.id });
    }
}

impl Tor {
    /// Starts Tor, or connects to it, and keeps it going in the background.
    pub fn start(source: Source) -> Self {
        let (state, state_rx) = watch::channel(TorState {
            phase: Phase::Starting,
            socks: None,
            restarts: 0,
            onion_services: 0,
            published: 0,
            onion_error: None,
        });
        let (requests, requests_rx) = mpsc::unbounded_channel();
        let supervisor = Supervisor {
            source,
            state,
            requests: requests_rx,
            services: BTreeMap::new(),
        };
        let task = tokio::spawn(supervisor.run());
        Self {
            state: state_rx,
            requests,
            next_service: Arc::default(),
            _task: Arc::new(AbortOnDropHandle::new(task)),
        }
    }

    pub fn state(&self) -> watch::Receiver<TorState> {
        self.state.clone()
    }

    /// Tor's SOCKS port, while Tor runs.
    pub fn socks(&self) -> Option<SocketAddr> {
        self.state.borrow().socks
    }

    /// Sets up an onion service for `key` whose streams to `port` go to
    /// `target`, and keeps it up until the returned value is dropped.
    pub(crate) fn serve(&self, key: SecretKey, port: u16, target: SocketAddr) -> OnionService {
        let id = self.next_service.fetch_add(1, Relaxed);
        let service = Service {
            key,
            port,
            target,
            published: false,
        };
        let _ = self.requests.send(Request::Add {
            id,
            service: Box::new(service),
        });
        OnionService {
            id,
            requests: self.requests.clone(),
        }
    }
}

struct Supervisor {
    source: Source,
    state: watch::Sender<TorState>,
    requests: mpsc::UnboundedReceiver<Request>,
    services: BTreeMap<u64, Service>,
}

impl Supervisor {
    async fn run(mut self) {
        let mut pause = PAUSES.0;
        loop {
            let started = Instant::now();
            let why = match self.session().await {
                Ok(()) => return,
                Err(e) => format!("{e:#}"),
            };
            if started.elapsed() >= STABLE {
                pause = PAUSES.0;
            }
            warn!(
                "Tor stopped: {why}; starting it again in {}s",
                pause.as_secs()
            );
            for service in self.services.values_mut() {
                service.published = false;
            }
            self.state.send_modify(|s| {
                s.phase = Phase::Down(why);
                s.socks = None;
                s.restarts += 1;
                s.published = 0;
            });
            let until = Instant::now() + pause;
            loop {
                tokio::select! {
                    _ = tokio::time::sleep_until(until) => break,
                    request = self.requests.recv() => match request {
                        Some(request) => self.note(request),
                        None => return,
                    },
                }
            }
            pause = (pause * 2).min(PAUSES.1);
        }
    }

    /// Starts Tor (or connects to it) and runs until it stops: `Ok` only
    /// when every handle to it is gone.
    async fn session(&mut self) -> Result<()> {
        self.state.send_modify(|s| s.phase = Phase::Starting);
        let (mut child, control_addr, cookie) = match &self.source {
            Source::Launch { binary, dir } => {
                let (child, addr) = launch(binary, dir).await?;
                (Some(child), addr, Some(cookie_file(dir)))
            }
            Source::External(addr) => (None, *addr, None),
        };
        let (mut control, mut events) = Control::connect(control_addr).await?;
        control.authenticate(cookie.as_deref()).await?;
        if child.is_some() {
            // Tor stops when this connection closes, so it never outlives
            // the node, or runs on without the node watching it.
            control.command("TAKEOWNERSHIP").await?;
        }
        control.command("SETEVENTS HS_DESC").await?;
        let socks = control.socks_addr().await?;
        for service in self.services.values() {
            self.add(&mut control, service).await;
        }
        self.state.send_modify(|s| {
            s.socks = Some(socks);
            s.phase = Phase::Connecting(0);
        });

        let mut next_check = Instant::now();
        loop {
            let exited = async {
                match child.as_mut() {
                    Some(child) => child.wait().await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                status = exited => match status {
                    Ok(status) => bail!("Tor exited ({status})"),
                    Err(e) => bail!("lost track of Tor: {e}"),
                },
                event = events.recv() => match event {
                    Some(event) => self.on_event(&event),
                    None => bail!("Tor closed its control port"),
                },
                request = self.requests.recv() => match request {
                    Some(request) => self.apply(request, &mut control).await,
                    None => return Ok(()),
                },
                _ = tokio::time::sleep_until(next_check) => {
                    let progress = control.bootstrap_progress().await?;
                    let phase = match progress {
                        100.. => Phase::Ready,
                        p => Phase::Connecting(p),
                    };
                    if phase == Phase::Ready && self.state.borrow().phase != Phase::Ready {
                        info!("Tor is connected to the Tor network");
                    }
                    next_check = Instant::now() + if phase == Phase::Ready {
                        CHECK_READY
                    } else {
                        CHECK_CONNECTING
                    };
                    self.state.send_if_modified(|s| {
                        let changed = s.phase != phase;
                        s.phase = phase;
                        changed
                    });
                }
            }
        }
    }

    /// Takes a request while Tor is down: it's carried out once Tor runs.
    fn note(&mut self, request: Request) {
        match request {
            Request::Add { id, service } => {
                self.services.insert(id, *service);
            }
            Request::Remove { id } => {
                self.services.remove(&id);
            }
        }
        self.count_services();
    }

    async fn apply(&mut self, request: Request, control: &mut Control) {
        match &request {
            Request::Add { service, .. } => self.add(control, service).await,
            Request::Remove { id } => {
                if let Some(service) = self.services.get(id) {
                    let id = onion::service_id(&service.key.public());
                    if let Err(e) = control.del_onion(&id).await {
                        warn!("couldn't remove an onion service: {e:#}");
                    }
                }
            }
        }
        self.note(request);
    }

    /// Notes when Tor has published one of the node's onion services:
    /// `HS_DESC UPLOADED <service id> ...`.
    fn on_event(&mut self, event: &Reply) {
        for line in &event.lines {
            let mut words = line.split(' ');
            if (words.next(), words.next()) != (Some("HS_DESC"), Some("UPLOADED")) {
                continue;
            }
            let Some(address) = words.next() else {
                continue;
            };
            for service in self.services.values_mut() {
                if !service.published && onion::service_id(&service.key.public()) == address {
                    service.published = true;
                    // Not which: a node's machine name mustn't be logged
                    // beside its public ID (ADR 0015).
                    info!("Tor published an onion service");
                }
            }
        }
        self.count_services();
    }

    fn count_services(&self) {
        let total = self.services.len();
        let published = self.services.values().filter(|s| s.published).count();
        self.state.send_if_modified(|s| {
            let changed = (s.onion_services, s.published) != (total, published);
            (s.onion_services, s.published) = (total, published);
            changed
        });
    }

    async fn add(&self, control: &mut Control, service: &Service) {
        let added = control
            .add_onion(&service.key, service.port, service.target)
            .await;
        if let Err(e) = &added {
            warn!("couldn't set up an onion service: {e:#}");
        }
        let error = added.err().map(|e| format!("{e:#}"));
        self.state.send_if_modified(|s| {
            let changed = s.onion_error != error;
            s.onion_error = error;
            changed
        });
    }
}

fn cookie_file(dir: &Path) -> PathBuf {
    dir.join("data").join("control_auth_cookie")
}

/// Starts Tor with its own settings in `dir`, and waits for its control
/// port.
async fn launch(binary: &Path, dir: &Path) -> Result<(Child, SocketAddr)> {
    let data = dir.join("data");
    std::fs::create_dir_all(&data).with_context(|| format!("creating {}", data.display()))?;
    for private in [dir, &data] {
        std::fs::set_permissions(private, std::fs::Permissions::from_mode(0o700))?;
    }
    let port_file = dir.join("control-port");
    let _ = std::fs::remove_file(&port_file);
    let torrc = dir.join("torrc");
    std::fs::write(&torrc, torrc_text(dir, std::process::id())?)?;
    // Settings from nowhere else: an empty file, not the system's defaults.
    let defaults = dir.join("torrc-defaults");
    std::fs::write(&defaults, "")?;

    let mut child = Command::new(binary)
        .arg("-f")
        .arg(&torrc)
        .arg("--defaults-torrc")
        .arg(&defaults)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => anyhow!(
                "{} isn't installed: install the tor package, or set [tor] binary in config.toml",
                binary.display()
            ),
            _ => anyhow!("starting {}: {e}", binary.display()),
        })?;
    let last = Arc::new(Mutex::new(VecDeque::new()));
    if let Some(out) = child.stdout.take() {
        tokio::spawn(forward_log(out, last.clone()));
    }
    if let Some(err) = child.stderr.take() {
        tokio::spawn(forward_log(err, last.clone()));
    }

    // Tor writes where its control port listens once it does.
    let give_up = Instant::now() + START_TIMEOUT;
    loop {
        if let Some(addr) = std::fs::read_to_string(&port_file)
            .ok()
            .and_then(|text| text.trim().strip_prefix("PORT=")?.parse().ok())
        {
            debug!(%addr, "Tor's control port is open");
            return Ok((child, addr));
        }
        if let Some(status) = child.try_wait()? {
            // Its last lines may still be on their way.
            tokio::time::sleep(Duration::from_millis(100)).await;
            let last = last.lock().unwrap_or_else(|e| e.into_inner());
            let last: Vec<&str> = last.iter().map(String::as_str).collect();
            bail!("Tor exited ({status}): {}", last.join(" / "));
        }
        if Instant::now() >= give_up {
            bail!(
                "Tor didn't open its control port within {}s",
                START_TIMEOUT.as_secs()
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Tor's settings: its own folder, ports the system picks, its control port
/// open only to programs that can read its cookie, and no relaying.
fn torrc_text(dir: &Path, pid: u32) -> Result<String> {
    let data = quote(&dir.join("data"))?;
    let port_file = quote(&dir.join("control-port"))?;
    let cookie = quote(&cookie_file(dir))?;
    Ok(format!(
        "# Written by the node each time it starts Tor: changes here are lost.\n\
         DataDirectory {data}\n\
         ControlPort auto\n\
         ControlPortWriteToFile {port_file}\n\
         CookieAuthentication 1\n\
         CookieAuthFile {cookie}\n\
         SocksPort auto ExtendedErrors\n\
         Log notice stdout\n\
         __OwningControllerProcess {pid}\n"
    ))
}

/// A path as a quoted torrc value.
fn quote(path: &Path) -> Result<String> {
    let text = path
        .to_str()
        .with_context(|| format!("{} isn't UTF-8", path.display()))?;
    if text.contains(['\n', '\r', '\0']) {
        bail!("{} has a line break in it", path.display());
    }
    Ok(format!(
        "\"{}\"",
        text.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

/// Passes Tor's log lines on: its warnings as warnings, the rest for
/// debugging. Keeps the last few, to say why Tor didn't start.
async fn forward_log(out: impl AsyncRead + Unpin, last: Arc<Mutex<VecDeque<String>>>) {
    let mut lines = BufReader::new(out).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        // "Nov 01 03:13:40.000 [notice] Bootstrapped 100% (done): Done"
        let message = line.split_once("] ").map_or(line.as_str(), |(_, m)| m);
        if line.contains("[warn]") || line.contains("[err]") {
            warn!("Tor: {message}");
        } else {
            debug!("Tor: {message}");
        }
        let mut last = last.lock().unwrap_or_else(|e| e.into_inner());
        if last.len() == LAST_LINES {
            last.pop_front();
        }
        last.push_back(message.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_quote_paths() {
        let text = torrc_text(Path::new("/home/a \"b\"/tor"), 42).unwrap();
        assert!(text.contains("DataDirectory \"/home/a \\\"b\\\"/tor/data\"\n"));
        assert!(text.contains("__OwningControllerProcess 42\n"));
        assert!(torrc_text(Path::new("/home/a\nControlPort 9051"), 1).is_err());
    }
}
