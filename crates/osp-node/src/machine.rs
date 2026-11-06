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
    collections::HashSet,
    fmt,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use iroh::{
    Endpoint, EndpointId, SecretKey,
    endpoint::{Connection, RecvStream, SendStream},
    protocol::{AcceptError, ProtocolHandler},
};
use osp_proto::{
    MACHINE_ALPN,
    framing::{read_frame_max, write_frame},
    machine::{
        ListingEntry, MAX_KNOWN, MAX_LISTING_PAGE, MAX_REPLY_LEN, MAX_REQUEST_LEN, MachineInfo,
        MachineRecord, MachineRequest, MachineResponse, OFFERS_DIRECTORY,
    },
    rpc::ClientResponse,
};
use osp_tor::TorTransport;
use tokio::sync::Semaphore;
use tracing::{debug, info};

use crate::{
    config::Config,
    db::Db,
    directory::DirectoryLists,
    home::Home,
    keys,
    meet::{Acquaintances, Stage, Talk},
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
    /// What its record says it offers other machines.
    offers: Vec<String>,
    pub endpoint: Endpoint,
    /// The machines it knows, and its record of each.
    pub acquaintances: Arc<Acquaintances>,
    /// Holds the onion service up, on Tor.
    _transport: Option<TorTransport>,
    #[cfg(feature = "sim")]
    sim_tor: Option<Arc<SimTor>>,
}

impl MachineSide {
    /// Starts the node's machine name, making its key if it's new: on Tor, or
    /// on the simulated network if that's asked for. Without either, or with
    /// `[machine]` off, the node has none. A new name starts again among
    /// machines as a stranger, knowing none of them.
    pub async fn start(
        home: &Home,
        config: &Config,
        tor: Option<&TorSide>,
        db: &Arc<Db>,
        sim: &SimSettings,
    ) -> Result<Option<Self>> {
        if !config.machine.enabled {
            return Ok(None);
        }
        let path = home.machine_key();
        let load = || match path.exists() {
            true => keys::load(&path),
            false => {
                db.forget_machines()?;
                keys::create(&path)
            }
        };
        let mut offers = Vec::new();
        if config.directory.serve {
            offers.push(OFFERS_DIRECTORY.to_string());
        }
        let started = if let Some(tor) = tor {
            let key = load()?;
            let (transport, endpoint) = tor.endpoint_for(key.clone(), MAX_STREAMS).await?;
            Some((key, endpoint, Some(transport)))
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
                    Some((key, endpoint, None))
                }
                _ => None,
            }
            #[cfg(not(feature = "sim"))]
            None
        };
        let Some((key, endpoint, transport)) = started else {
            return Ok(None);
        };
        // This machine and the directories it was told of are kept apart from
        // the machines it meets (ADR 0016).
        let apart: HashSet<EndpointId> = config
            .directory
            .known
            .iter()
            .map(|directory| directory.machine)
            .chain([key.public()])
            .collect();
        let acquaintances = Acquaintances::load(db.clone(), apart, sim.meeting)?;
        info!("the machine name is on");
        Ok(Some(Self {
            key,
            offers,
            endpoint,
            acquaintances: Arc::new(acquaintances),
            _transport: transport,
            #[cfg(feature = "sim")]
            sim_tor: sim.tor_delay.map(|delay| Arc::new(SimTor::new(delay))),
        }))
    }

    pub fn id(&self) -> EndpointId {
        self.key.public()
    }

    /// With `lists`, it serves as a directory too.
    pub fn protocol(&self, lists: Option<DirectoryLists>) -> MachineProtocol {
        MachineProtocol {
            key: self.key.clone(),
            offers: self.offers.clone(),
            connections: Arc::new(Semaphore::new(MAX_CONNECTIONS)),
            lists,
            acquaintances: self.acquaintances.clone(),
            #[cfg(feature = "sim")]
            sim_tor: self.sim_tor.clone(),
        }
    }

    /// Estimated Tor circuits this machine has built, on the simulated
    /// network with Tor-like delays.
    #[cfg(feature = "sim")]
    pub fn circuits(&self) -> Option<u64> {
        let sim = self.sim_tor.as_ref()?;
        Some(sim.circuits.load(std::sync::atomic::Ordering::Relaxed))
    }

    /// A conversation with a machine to meet it or check in: a greeting, then
    /// `Known`. A machine that doesn't know `Known` yet has still answered.
    pub async fn talk(&self, machine: EndpointId) -> Result<Talk> {
        let conn = self.connect(machine).await?;
        let talked = async {
            let record = self.ask_hello(&conn, machine).await?;
            checked(&record, machine)?;
            let (known, too_many) = match self.ask(&conn, machine, &MachineRequest::Known).await {
                Ok(MachineResponse::Known { mut records }) => {
                    let too_many = records.len() > MAX_KNOWN;
                    records.truncate(MAX_KNOWN);
                    (records, too_many)
                }
                _ => (Vec::new(), false),
            };
            Ok(Talk {
                record,
                known,
                too_many,
            })
        }
        .await;
        close(&conn);
        talked
    }

    /// Greets another machine, and closes the connection: a conversation is
    /// an event, not something kept open.
    pub async fn hello(&self, machine: EndpointId) -> Result<Greeting> {
        let started = Instant::now();
        let conn = self.connect(machine).await?;
        let connected = started.elapsed();
        let started = Instant::now();
        let reply = self.ask_hello(&conn, machine).await;
        close(&conn);
        let record = reply?;
        Ok(Greeting {
            info: checked(&record, machine)?,
            record,
            connected,
            asked: started.elapsed(),
        })
    }

    /// Starts a conversation with another machine, for the caller to `close`.
    pub async fn connect(&self, machine: EndpointId) -> Result<Connection> {
        #[cfg(feature = "sim")]
        if let Some(sim) = &self.sim_tor {
            sim.connecting(self.id(), machine).await;
        }
        tokio::time::timeout(
            CONNECT_TIMEOUT,
            self.endpoint.connect(machine, MACHINE_ALPN),
        )
        .await
        .with_context(|| format!("reaching machine {} timed out", words(&machine)))?
        .with_context(|| format!("reaching machine {}", words(&machine)))
    }

    /// Greets the machine at the other end of a conversation already started.
    pub async fn hello_on(&self, conn: &Connection, machine: EndpointId) -> Result<MachineInfo> {
        checked(&self.ask_hello(conn, machine).await?, machine)
    }

    /// Asks a directory for a page of its lists.
    pub async fn listing(
        &self,
        conn: &Connection,
        directory: EndpointId,
        since: u64,
        limit: u32,
    ) -> Result<(Vec<ListingEntry>, u64)> {
        let request = MachineRequest::Listing { since, limit };
        match self.ask(conn, directory, &request).await? {
            MachineResponse::Listing { entries, latest } => Ok((entries, latest)),
            MachineResponse::Error { message } => bail!("it answered: {message}"),
            _ => bail!("it answered something else"),
        }
    }

    async fn ask(
        &self,
        conn: &Connection,
        #[cfg_attr(not(feature = "sim"), allow(unused_variables))] machine: EndpointId,
        request: &MachineRequest,
    ) -> Result<MachineResponse> {
        let (mut send, mut recv) = conn.open_bi().await?;
        write_frame(&mut send, request).await?;
        send.finish()?;
        let reply = tokio::time::timeout(REQUEST_TIMEOUT, read_frame_max(&mut recv, MAX_REPLY_LEN))
            .await
            .context("no reply in time")??;
        #[cfg(feature = "sim")]
        if let Some(sim) = &self.sim_tor {
            let bytes = serde_json::to_vec(&reply).map_or(0, |v| v.len());
            tokio::time::sleep(sim.delay.request(&self.id(), &machine, bytes)).await;
        }
        Ok(reply)
    }

    async fn ask_hello(&self, conn: &Connection, machine: EndpointId) -> Result<MachineRecord> {
        let request = MachineRequest::Hello {
            record: own_record(&self.key, &self.offers)?,
        };
        match self.ask(conn, machine, &request).await? {
            MachineResponse::Hello { record } => Ok(record),
            MachineResponse::Error { message } => bail!("it answered: {message}"),
            _ => bail!("it answered something else"),
        }
    }
}

/// Ends a conversation: it's an event, not something kept open.
pub(crate) fn close(conn: &Connection) {
    conn.close(CLOSE_DONE.into(), b"done");
}

/// Tor-like delays on the simulated network, which has no Tor, and an
/// estimate of the circuits Tor would build. Each conversation with an onion
/// service builds two at the machine asking, one to an introduction point and
/// one to a rendezvous point, and one at the machine asked, to that rendezvous
/// point. Conversations are events hours apart, so none reuses another's, as
/// Tor stops putting new streams on a circuit ten minutes after its first.
/// Not counted: the few circuits an onion service holds to its own
/// introduction points all the time.
#[cfg(feature = "sim")]
pub(crate) struct SimTor {
    delay: crate::simnet::TorDelay,
    circuits: std::sync::atomic::AtomicU64,
}

#[cfg(feature = "sim")]
impl SimTor {
    fn new(delay: crate::simnet::TorDelay) -> Self {
        Self {
            delay,
            circuits: Default::default(),
        }
    }

    /// Waits as connecting to `machine` over Tor would: for new circuits,
    /// then a round trip.
    async fn connecting(&self, me: EndpointId, machine: EndpointId) {
        self.circuits
            .fetch_add(2, std::sync::atomic::Ordering::Relaxed);
        tokio::time::sleep(self.delay.first_connect()).await;
        tokio::time::sleep(self.delay.round_trip(&me, &machine)).await;
    }

    /// A machine reached this one.
    fn reached_by(&self) {
        self.circuits
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

/// What greeting a machine brought.
pub(crate) struct Greeting {
    /// Its record, as it signed it.
    pub record: MachineRecord,
    /// What that says.
    pub info: MachineInfo,
    pub connected: Duration,
    pub asked: Duration,
}

/// This machine's record, freshly dated.
fn own_record(key: &SecretKey, offers: &[String]) -> Result<MachineRecord> {
    MachineRecord::new(key, offers.to_vec(), osp_proto::now_rfc3339())
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
    offers: Vec<String>,
    connections: Arc<Semaphore>,
    /// A directory's lists, on a machine that is one.
    lists: Option<DirectoryLists>,
    /// Who greets it is heard of, and `Known` comes from these.
    acquaintances: Arc<Acquaintances>,
    #[cfg(feature = "sim")]
    sim_tor: Option<Arc<SimTor>>,
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
        #[cfg(feature = "sim")]
        if let Some(sim) = &self.sim_tor {
            sim.reached_by();
        }
        // Anyone can make a name for free, so one gets a short, bounded turn.
        let _ = tokio::time::timeout(CONNECTION_LIMIT, async {
            let remote = conn.remote_id();
            while let Ok((send, recv)) = conn.accept_bi().await {
                let served =
                    tokio::time::timeout(REQUEST_TIMEOUT, self.serve_stream(remote, send, recv))
                        .await
                        .unwrap_or_else(|_| Err(anyhow::anyhow!("took too long")));
                // Not who: a log holds the public ID too.
                if let Err(e) = served {
                    debug!("machine request failed: {e:#}");
                }
            }
        })
        .await;
        Ok(())
    }
}

impl MachineProtocol {
    async fn serve_stream(
        &self,
        remote: EndpointId,
        mut send: SendStream,
        mut recv: RecvStream,
    ) -> Result<()> {
        let request: MachineRequest = read_frame_max(&mut recv, MAX_REQUEST_LEN).await?;
        let reply = match request {
            MachineRequest::Hello { record } => match checked(&record, remote) {
                Ok(_) => {
                    if let Some(lists) = &self.lists {
                        lists.greeted(remote);
                    }
                    self.acquaintances.greeted(&record);
                    MachineResponse::Hello {
                        record: own_record(&self.key, &self.offers)?,
                    }
                }
                Err(e) => MachineResponse::Error {
                    message: format!("{e:#}"),
                },
            },
            MachineRequest::Listing { since, limit } => match &self.lists {
                Some(lists) => {
                    let (entries, latest) = lists.listing(since, limit.min(MAX_LISTING_PAGE))?;
                    MachineResponse::Listing { entries, latest }
                }
                None => MachineResponse::Error {
                    message: "this machine isn't a directory".to_string(),
                },
            },
            // Only what the machine itself met: nothing the owner does changes it.
            MachineRequest::Known => MachineResponse::Known {
                records: self.acquaintances.known_reply(&remote),
            },
            MachineRequest::Unknown => MachineResponse::Error {
                message: "this machine doesn't know that request".to_string(),
            },
        };
        write_frame(&mut send, &reply).await?;
        send.finish()?;
        Ok(())
    }
}

impl Core {
    pub(crate) fn machine(&self) -> Result<&MachineSide> {
        self.machine.as_ref().context(
            "this node has no machine name: it needs enabled = true under [tor] and under [machine] in config.toml",
        )
    }

    /// This node's machine name, for its owner.
    pub(crate) fn machine_name(&self) -> Result<ClientResponse> {
        let side = self.machine()?;
        let machine = side.id();
        let known = side.acquaintances.acquaintances();
        let count = |stage| known.iter().filter(|a| a.stage == stage).count() as u64;
        Ok(ClientResponse::MachineName {
            words: words(&machine),
            machine,
            heard_of: count(Stage::Heard),
            met: count(Stage::Met),
            known: count(Stage::Known),
        })
    }

    /// Greets another machine, for `osp machine hello`.
    pub(crate) async fn machine_hello(&self, machine: EndpointId) -> Result<ClientResponse> {
        let side = self.machine()?;
        if let Some(tor) = &self.tor {
            tor.check_ready()?;
        }
        let greeting = side.hello(machine).await?;
        Ok(ClientResponse::MachineGreeted {
            words: words(&machine),
            offers: greeting.info.offers,
            connect_ms: greeting.connected.as_millis() as u64,
            round_trip_ms: greeting.asked.as_millis() as u64,
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
        checked(&own_record(&key, &[]).unwrap(), me).unwrap();

        let other = SecretKey::from_bytes(&[8; 32]).public();
        let why = checked(&own_record(&key, &[]).unwrap(), other).unwrap_err();
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
