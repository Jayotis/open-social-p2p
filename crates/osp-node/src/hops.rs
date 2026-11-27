//! The viewer's side of a circuit (ADR 0043 and its design note, steps 2 to
//! 4): picking its own hop, and fetching through it and the host's.
//!
//! Say Jimmy opens Alice's photo. Alice's node named its hop Q, a slot and
//! its circuit endpoint (`CircuitOffer`). Jimmy's node picks a hop of its own,
//! P: a machine with standing with it, or a seed carrier (`picks`), and
//! sometimes a machine in the middle, M. It dials P from an endpoint made for
//! this circuit, builds V → P → (M →) Q, hands Q the slot, and fetches over
//! the ends' own QUIC on an endpoint whose only path is the circuit. If both
//! ends picked the same machine, it's one hop (ADR 0043 §4).
//!
//! A circuit that can't be built, or a fetch that fails over it, leaves the
//! fetch to the way it went before: through the relays, the project's first
//! (ADR 0044). Never directly to Alice's node, whose address Jimmy's never
//! learns.

use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex, Weak},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};
use iroh::{Endpoint, EndpointId, SecretKey, TransportAddr, endpoint::Connection};
use iroh_blobs::util::connection_pool::{ConnectionPool, ConnectionRef};
use osp_carrier::{CarrierInfo, CircuitCloser, Descriptor, SETUP_TIMEOUT, ViewerCircuit};
use osp_proto::{
    BlobHash,
    framing::{read_frame, write_frame},
    social::{
        CARRIER_DESCRIPTOR_ALPN, CarrierDescriptorRequest, CarrierDescriptorResponse, CircuitOffer,
    },
};
use tracing::{debug, info};

use crate::{
    carrier::{relay_mode, relayed_endpoint},
    db::BlobClass,
    machine::{carrier_of, descriptor_from_text, descriptor_text},
    node::Core,
    picks::{Known, Middle},
    store::{self, Source, Sources},
    viewing::in_own_order,
};

/// Nodes that offered a way to their own media through their hops, each
/// with its offer.
pub(crate) type Offers = Vec<(EndpointId, CircuitOffer)>;

/// A cached descriptor is asked for again this long before its circuit key
/// runs out, so a circuit isn't built on a key about to go.
const FRESH_FOR: u64 = 5 * 60;

/// How long a carrier endpoint is given to say its descriptor, before its
/// machine is asked over Tor instead. A relay's round trip, normally.
const ASK_CARRIER_WAIT: Duration = Duration::from_secs(10);

/// With a machine in the middle, its circuit is built alongside the one
/// without it, and used if it's ready within this: stage D's mark for a
/// circuit to be usable (`docs/testing/carrier-lab.md`). After it, whichever
/// is ready first carries, so a middle that's gone or slow costs no more
/// than the circuit without it. Each still has the carriers' whole
/// `SETUP_TIMEOUT` to answer.
const PREFER_MIDDLE_FOR: Duration = Duration::from_secs(5);

/// A link this node's own hop is kept for, at most (`OwnLink`): then its
/// key is replaced.
const KEEP_LINK_FOR: Duration = Duration::from_secs(24 * 60 * 60);
/// How often the kept link is looked at, and made again if it's gone.
const KEEP_LINK_CHECK: Duration = Duration::from_secs(60);

/// A circuit ready to fetch over: its link to the first hop, what ends it,
/// and the endpoint whose only path is the circuit.
struct Circuit {
    link: Link,
    closer: CircuitCloser,
    ends: Endpoint,
}

/// A link to a circuit's first hop: the endpoint it was dialled from, with a
/// key made for it and published nowhere, and the connection.
pub(crate) struct Link {
    endpoint: Endpoint,
    conn: Connection,
    /// The first hop's machine, and its carrier endpoint.
    machine: EndpointId,
    to: EndpointId,
    made: tokio::time::Instant,
    /// This node's addresses when it was made (`Core::own_addrs`).
    addrs: Vec<SocketAddr>,
    /// Held while the link is: `OwnLink` tells a lent link by it.
    token: Arc<()>,
}

impl Link {
    /// Whether it can carry another circuit: still open, younger than
    /// `KEEP_LINK_FOR`, and made at the addresses this node has now.
    fn usable(&self, addrs: &[SocketAddr]) -> bool {
        self.conn.close_reason().is_none()
            && self.made.elapsed() < KEEP_LINK_FOR
            && self.addrs == addrs
    }

    async fn close(self) {
        self.conn.close(0u32.into(), b"done");
        self.endpoint.close().await;
    }
}

/// The link to this node's own hop, kept open between circuits (the
/// maintainer's choice after stage D's fifth run, 2026-11-23). Say Jimmy's
/// node fetches a video through carriers: about half of building the
/// circuit was reaching his own hop P (the relay dial, QUIC's handshake and
/// the wait for room), the same for every fetch. With the link kept, a
/// circuit starts with `CREATE`. P sees Jimmy's address on each fetch
/// anyway; it can now tie his fetches to one key, and sees when his node is
/// up, so the key is replaced each day and when his addresses change. One
/// circuit uses it at a time; another at once dials a link of its own.
#[derive(Default)]
pub(crate) struct OwnLink {
    kept: Mutex<Option<Link>>,
    /// The kept link while a circuit has it: none is made beside it.
    lent: Mutex<Weak<()>>,
    /// Circuits built on the kept link since the node started.
    used: std::sync::atomic::AtomicUsize,
}

impl OwnLink {
    /// The machine of the hop the link is kept to, while it's kept.
    pub(crate) fn machine(&self) -> Option<EndpointId> {
        self.kept.lock().unwrap().as_ref().map(|l| l.machine)
    }

    /// How many circuits were built on the kept link since the node started.
    pub(crate) fn used(&self) -> usize {
        self.used.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Closes the kept link, as the node stops.
    pub(crate) async fn close(&self) {
        let kept = self.kept.lock().unwrap().take();
        if let Some(link) = kept {
            link.close().await;
        }
    }
}

/// A circuit built to an offer, with what fetching over it needs: the hops
/// without the middle, for a second try and for credit; the middle, if one
/// was picked, and whether the circuit goes through it; and this node's own
/// hop, whose descriptor is forgotten if the fetch fails. And the
/// connection to the host over it, if it was opened ahead of the fetch.
struct Built {
    circuit: Circuit,
    hops: Vec<Descriptor>,
    middle: Option<Middle>,
    through_middle: bool,
    known: Known,
    my_hop: EndpointId,
    connected: Option<Connected>,
}

/// A connection to the host over a circuit, opened before anything is asked
/// for over it: the pool it's in, which the fetch uses, and a hold on it, so
/// the pool doesn't close it as unused meanwhile.
struct Connected {
    pool: ConnectionPool,
    _held: ConnectionRef,
}

/// How the two builds `race` waited on came out.
struct Raced<T> {
    /// The circuit to fetch over, and whether it's the one with the middle.
    chosen: Result<(T, bool)>,
    /// The other, if it was built too: to close.
    unused: Option<T>,
    /// Why the circuit through the middle failed, if it did. Not set when
    /// it was only slower.
    middle_failed: Option<anyhow::Error>,
}

/// Waits on `with` the middle and `without` it, both started: `with` if it's
/// ready within `prefer_for`, else the first ready after that. One that
/// fails leaves the other to wait for. The one not chosen is dropped if
/// it's still building.
async fn race<T>(
    with: impl Future<Output = Result<T>>,
    without: impl Future<Output = Result<T>>,
    prefer_for: Duration,
) -> Raced<T> {
    tokio::pin!(with, without);
    let preferred = tokio::time::sleep(prefer_for);
    tokio::pin!(preferred);
    let (mut w, mut wo): (Option<Result<T>>, Option<Result<T>>) = (None, None);
    let mut waited = false;
    loop {
        match (&w, &wo) {
            (Some(Ok(_)), _) | (Some(Err(_)), Some(_)) => break,
            (None, Some(Ok(_))) if waited => break,
            _ => {}
        }
        tokio::select! {
            r = &mut with, if w.is_none() => w = Some(r),
            r = &mut without, if wo.is_none() => wo = Some(r),
            _ = &mut preferred, if !waited => waited = true,
        }
    }
    match (w, wo) {
        (Some(Ok(circuit)), other) => Raced {
            chosen: Ok((circuit, true)),
            unused: other.and_then(Result::ok),
            middle_failed: None,
        },
        (w, Some(Ok(circuit))) => Raced {
            chosen: Ok((circuit, false)),
            unused: None,
            middle_failed: w.and_then(Result::err),
        },
        (w, wo) => Raced {
            chosen: Err(wo
                .and_then(Result::err)
                .unwrap_or_else(|| anyhow!("no circuit was built"))),
            unused: None,
            middle_failed: w.and_then(Result::err),
        },
    }
}

/// Waits on `tried` with `building` started beside it. If `tried` succeeds,
/// what was built by then, to close, with a build still going dropped; if it
/// fails, the build, waited for if it isn't done. So a try that fails costs
/// no more than the build alone.
async fn try_while_building<T, B>(
    tried: impl Future<Output = Result<T>>,
    building: impl Future<Output = B>,
) -> (Result<T>, Option<B>) {
    tokio::pin!(tried, building);
    let mut built = None;
    let tried = loop {
        tokio::select! {
            r = &mut tried => break r,
            b = &mut building, if built.is_none() => built = Some(b),
        }
    };
    if tried.is_err() && built.is_none() {
        built = Some(building.await);
    }
    (tried, built)
}

/// Carriers' descriptors, each with when its circuit key runs out. `fresh`
/// holds what this node goes by, until a refused circuit forgets it. `held`
/// is the last one of each, kept after that: what names the carrier
/// endpoint to ask for its descriptor now (`Core::hop_descriptor`). The seed
/// carriers' are kept in `file` across restarts too, as a restarted node
/// otherwise has only Tor to ask, and Tor can be minutes behind a carrier
/// that restarted.
#[derive(Default)]
pub(crate) struct SeedDescriptors {
    fresh: Mutex<HashMap<EndpointId, (Descriptor, u64)>>,
    held: Mutex<HashMap<EndpointId, (Descriptor, u64)>>,
    file: Option<(PathBuf, Vec<EndpointId>)>,
}

impl SeedDescriptors {
    /// The seed carriers `seeds`' descriptors kept in `path`, if any. A line
    /// is a machine, when its key runs out and the descriptor's text; one
    /// that doesn't check out is left out.
    pub(crate) fn load(path: PathBuf, seeds: Vec<EndpointId>) -> Self {
        let mut held = HashMap::new();
        for line in std::fs::read_to_string(&path).unwrap_or_default().lines() {
            let mut words = line.split(' ');
            let (Some(machine), Some(until), Some(text)) =
                (words.next(), words.next(), words.next())
            else {
                continue;
            };
            let (Ok(machine), Ok(until)) = (machine.parse::<EndpointId>(), until.parse::<u64>())
            else {
                continue;
            };
            if seeds.contains(&machine)
                && let Some(descriptor) = descriptor_from_text(text, &machine, until)
            {
                held.insert(machine, (descriptor, until));
            }
        }
        Self {
            fresh: Mutex::default(),
            held: Mutex::new(held),
            file: Some((path, seeds)),
        }
    }

    /// `machine`'s descriptor to go by, if its key is good past `until`.
    fn fresh(&self, machine: &EndpointId, until: u64) -> Option<Descriptor> {
        let fresh = self.fresh.lock().unwrap();
        let (descriptor, valid_until) = fresh.get(machine)?;
        (*valid_until > until).then(|| descriptor.clone())
    }

    /// The last descriptor held for `machine`, however old, with when its
    /// key runs out.
    fn held(&self, machine: &EndpointId) -> Option<(Descriptor, u64)> {
        self.held.lock().unwrap().get(machine).cloned()
    }

    /// Goes by `descriptor` for `machine` from now on, and keeps it.
    fn keep(&self, machine: EndpointId, descriptor: &Descriptor, until: u64) {
        let entry = (descriptor.clone(), until);
        self.fresh.lock().unwrap().insert(machine, entry.clone());
        let mut held = self.held.lock().unwrap();
        if held
            .insert(machine, entry)
            .is_some_and(|(_, was)| was == until)
        {
            return;
        }
        let Some((path, seeds)) = &self.file else {
            return;
        };
        if !seeds.contains(&machine) {
            return;
        }
        let text: String = held
            .iter()
            .filter(|(m, _)| seeds.contains(m))
            .map(|(m, (d, until))| format!("{m} {until} {}\n", descriptor_text(d)))
            .collect();
        if let Err(e) = std::fs::write(path, text) {
            debug!("couldn't keep the seed carriers' descriptors: {e:#}");
        }
    }

    /// Goes by `machine`'s descriptor no more, but keeps it held.
    fn forget(&self, machine: &EndpointId) {
        self.fresh.lock().unwrap().remove(machine);
    }
}

/// Files whose hosts are being asked for circuit offers now, so a second
/// fetch of one, such as an open while it's fetched ahead of time, doesn't
/// ask again (`OffersAsked::start`).
#[derive(Default)]
pub(crate) struct OffersAsked(Mutex<HashSet<BlobHash>>);

impl OffersAsked {
    /// Notes that `blob`'s hosts are asked, until the guard is dropped.
    /// None if they're being asked already.
    pub(crate) fn start(&self, blob: BlobHash) -> Option<OffersAsking<'_>> {
        let mut asked = self.0.lock().unwrap_or_else(|e| e.into_inner());
        asked
            .insert(blob)
            .then(|| OffersAsking { asked: self, blob })
    }
}

pub(crate) struct OffersAsking<'a> {
    asked: &'a OffersAsked,
    blob: BlobHash,
}

impl Drop for OffersAsking<'_> {
    fn drop(&mut self) {
        let mut asked = self.asked.0.lock().unwrap_or_else(|e| e.into_inner());
        asked.remove(&self.blob);
    }
}

pub(crate) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Q's descriptor in an offer, checked.
fn offered_hop(offer: &CircuitOffer, now: u64) -> Result<Descriptor> {
    let bytes = data_encoding::BASE64URL_NOPAD
        .decode(offer.carrier.as_bytes())
        .map_err(|_| anyhow!("the offer's descriptor isn't base64url"))?;
    let descriptor = Descriptor::from_bytes(&bytes)?;
    descriptor.verify(now)?;
    Ok(descriptor)
}

impl Core {
    /// Whether this node fetches through carriers: its owner said the hops
    /// it picks may see its address, it knows seed carriers, which its own
    /// hop is picked from, and it has a machine name to read their records
    /// with.
    pub(crate) fn fetches_through_carriers(&self) -> bool {
        self.uses_hops()
            && !self.config.carrier.known.is_empty()
            && self.machine.is_some()
            && self.config.network.relay != crate::config::RelayChoice::Disabled
    }

    /// Whether the owner said yes to the hops it picks seeing its address.
    pub(crate) fn uses_hops(&self) -> bool {
        *self.hops.borrow() == Some(true)
    }

    /// The descriptor the seed carrier `machine`'s record names: with
    /// `cached`, the one held while its circuit key has a while to go; else,
    /// or without one, asked for over Tor. A carrier that restarts has a new
    /// circuit key, so a circuit refused through it forgets the one held.
    pub(crate) async fn seed_descriptor(
        &self,
        machine: EndpointId,
        cached: bool,
    ) -> Result<Descriptor> {
        if cached
            && let Some(descriptor) = self
                .seed_descriptors
                .fresh(&machine, unix_now() + FRESH_FOR)
        {
            return Ok(descriptor);
        }
        let greeting = self.machine()?.hello(machine).await?;
        let descriptor = carrier_of(&greeting.info, unix_now())
            .context("its machine record names no carrier that checks out")?;
        let until = descriptor.verify(unix_now())?.valid_until;
        self.seed_descriptors.keep(machine, &descriptor, until);
        Ok(descriptor)
    }

    /// The descriptor of `machine`, which this node is about to link or
    /// build a circuit through: as [`seed_descriptor`](Self::seed_descriptor),
    /// but asked first of the carrier endpoint the last one held names,
    /// through its relay. Say Alice's hop Q restarted: the machines that
    /// greet it over Tor can't reach it for minutes (their Tor holds its old
    /// onion descriptor), but its carrier endpoint keeps its key and is back
    /// on its relay in seconds. The link or circuit shows Q this node's
    /// address anyway, so the ask through the relay gives away nothing more.
    pub(crate) async fn hop_descriptor(
        &self,
        machine: EndpointId,
        cached: bool,
    ) -> Result<Descriptor> {
        // Stage D's sixth run had 9 s before a circuit's build with nothing
        // logged: each way here says where the descriptor came from, and
        // how long it took.
        let hop = crate::machine::words(&machine);
        let started = tokio::time::Instant::now();
        let ms = || started.elapsed().as_millis() as u64;
        if cached
            && let Some(descriptor) = self
                .seed_descriptors
                .fresh(&machine, unix_now() + FRESH_FOR)
        {
            debug!(%hop, from = "cache", ms = ms(), "a hop's descriptor");
            return Ok(descriptor);
        }
        if let Some(held) = self.seed_descriptors.held(&machine) {
            match tokio::time::timeout(ASK_CARRIER_WAIT, self.ask_carrier(machine, &held)).await {
                Ok(Ok((descriptor, until))) => {
                    self.seed_descriptors.keep(machine, &descriptor, until);
                    debug!(%hop, from = "carrier", ms = ms(), "a hop's descriptor");
                    return Ok(descriptor);
                }
                Ok(Err(e)) => {
                    debug!(hop = %crate::machine::words(&machine), "its carrier endpoint didn't say its descriptor; over Tor instead: {e:#}")
                }
                Err(_) => {
                    debug!(hop = %crate::machine::words(&machine), "its carrier endpoint didn't say its descriptor in time; over Tor instead")
                }
            }
        }
        let over_tor = self.seed_descriptor(machine, false).await;
        match &over_tor {
            Ok(_) => debug!(%hop, from = "tor", ms = ms(), "a hop's descriptor"),
            Err(e) => debug!(%hop, ms = ms(), "a hop's descriptor couldn't be had: {e:#}"),
        }
        over_tor
    }

    /// Asks the carrier endpoint `held` names, from a key made for the ask,
    /// for `machine`'s descriptor now, and checks it: signed by `machine`,
    /// good now, and naming the endpoint that said it.
    async fn ask_carrier(
        &self,
        machine: EndpointId,
        (held, until): &(Descriptor, u64),
    ) -> Result<(Descriptor, u64)> {
        // It may have run out, but it still names the endpoint: its
        // signature is checked at a time it was good.
        let info = held.verify(*until)?;
        anyhow::ensure!(
            info.machine == machine,
            "a held descriptor for another machine"
        );
        let relay: iroh::RelayUrl = info.relay.parse().context("its descriptor's relay")?;
        let relays = self.tracker.relays().clone();
        anyhow::ensure!(
            relays.permits(&TransportAddr::Relay(relay.clone())),
            "a relay this node doesn't use: {relay}"
        );
        let mode = relay_mode(&self.config)?.context("relays are off")?;
        let endpoint =
            relayed_endpoint(&self.config, mode, &relays, SecretKey::generate(), vec![]).await?;
        let asked = async {
            let addr = iroh::EndpointAddr::new(info.endpoint).with_relay_url(relay);
            let conn = endpoint.connect(addr, CARRIER_DESCRIPTOR_ALPN).await?;
            let (mut send, mut recv) = conn.open_bi().await?;
            write_frame(&mut send, &CarrierDescriptorRequest {}).await?;
            send.finish()?;
            let resp: CarrierDescriptorResponse = read_frame(&mut recv).await?;
            conn.close(0u32.into(), b"done");
            anyhow::Ok(resp.carrier)
        }
        .await;
        endpoint.close().await;
        let now = unix_now();
        let descriptor = descriptor_from_text(&asked?, &machine, now)
            .context("the descriptor it said doesn't check out for its machine")?;
        let now_info = descriptor.verify(now)?;
        anyhow::ensure!(
            now_info.endpoint == info.endpoint,
            "it said a descriptor for another endpoint"
        );
        Ok((descriptor, now_info.valid_until))
    }

    /// The seed carriers known, in an order of this node's own, so ends
    /// spread over them.
    pub(crate) fn seeds_in_own_order(&self) -> Vec<EndpointId> {
        let mut seeds: Vec<EndpointId> = self
            .config
            .carrier
            .known
            .iter()
            .map(|s| s.machine)
            .collect();
        in_own_order(&mut seeds);
        seeds
    }

    /// Fetches `blob` from `host`'s node through a circuit to what `offer`
    /// names: this node's own hop, perhaps a machine in the middle (the
    /// `picks` module), then the host's hop, then the host's circuit
    /// endpoint. Nothing goes any other way. With a machine in the middle,
    /// the circuit without it is built at the same time, and carries the
    /// fetch if the middle's is late or fails (`open_preferring_middle`); a
    /// fetch over the middle's that fails is tried once more without it. A
    /// fetch that passes the hash check credits the hops that carried it.
    pub(crate) async fn fetch_over_circuit(
        &self,
        blob: &BlobHash,
        host: EndpointId,
        offer: &CircuitOffer,
        class: BlobClass,
        declared: u64,
    ) -> Result<u64> {
        let built = self.circuit_to(blob, offer).await?;
        self.fetch_over_built(built, blob, host, offer, (class, declared))
            .await
    }

    /// The circuit `fetch_over_circuit` fetches over, built and ready.
    async fn circuit_to(&self, blob: &BlobHash, offer: &CircuitOffer) -> Result<Built> {
        let theirs = offered_hop(offer, unix_now())?;
        let their_hop = theirs.verify(unix_now())?.machine;
        let known = self.known_carriers()?;
        let mine = self.own_hop(&known, Some(their_hop)).await?;
        let my_hop = mine.verify(unix_now())?.machine;
        let hops = match my_hop == their_hop {
            // Both ends picked it: one hop, and nothing in the middle, which
            // would sit next to an end.
            true => vec![theirs],
            false => vec![mine, theirs],
        };
        let middle = match hops.len() {
            2 => self.middle_hop(&known, &[my_hop, their_hop]),
            _ => None,
        };
        let opened = match &middle {
            None => self.open(blob, offer, &hops).await.map(|c| (c, false)),
            Some(m) => {
                let mut through = hops.clone();
                through.insert(1, m.descriptor.clone());
                // Boxed: two circuits' builds at once are a large future.
                Box::pin(self.open_preferring_middle(blob, offer, &through, &hops, m)).await
            }
        };
        let (circuit, through_middle) = opened.inspect_err(|_| self.forget_descriptor(&my_hop))?;
        Ok(Built {
            circuit,
            hops,
            middle,
            through_middle,
            known,
            my_hop,
            connected: None,
        })
    }

    /// A seed carrier's descriptor is asked for again next time, in case the
    /// hop restarted. Only after a refused build: a download that broke on a
    /// circuit the descriptor built says nothing against it, and asking
    /// again costs seconds.
    fn forget_descriptor(&self, machine: &EndpointId) {
        self.seed_descriptors.forget(machine);
    }

    /// Fetches `blob` from `host` over `built`, then credits the hops that
    /// carried it, as `fetch_over_circuit` says.
    async fn fetch_over_built(
        &self,
        built: Built,
        blob: &BlobHash,
        host: EndpointId,
        offer: &CircuitOffer,
        job: (BlobClass, u64),
    ) -> Result<u64> {
        let Built {
            circuit,
            mut hops,
            middle,
            through_middle,
            known,
            my_hop,
            connected,
        } = built;
        let (_, declared) = job;
        let fetched = match (&middle, through_middle) {
            (Some(m), true) => match self
                .download_over(circuit, connected, blob, host, offer, job)
                .await
            {
                Ok(size) => Ok((size, true)),
                Err(e) => {
                    debug!(machine = %m.machine.fmt_short(), "the fetch through a machine in the middle failed; without it: {e:#}");
                    self.middle_failed(m);
                    self.fetch_through_hops(blob, host, offer, &hops, &my_hop, job)
                        .await
                        .map(|size| (size, false))
                }
            },
            _ => self
                .download_over(circuit, connected, blob, host, offer, job)
                .await
                .map(|size| (size, false)),
        };
        let (size, through_middle) = fetched?;
        // Only the hops that carried it are credited.
        let middle = middle.filter(|_| through_middle);
        if let Some(m) = &middle {
            hops.insert(1, m.descriptor.clone());
        }
        let machines: Vec<EndpointId> = hops
            .iter()
            .filter_map(|d| d.verify(unix_now()).ok().map(|i| i.machine))
            .collect();
        if declared > store::SMALL_BLOB
            && let Err(e) = self.credit_hops(&known, &machines, middle.as_ref())
        {
            debug!("couldn't note the hops' job: {e:#}");
        }
        info!(
            blob = %blob.fmt_short(),
            hops = hops.len(),
            "fetched through carriers"
        );
        self.may_ask_for_work();
        Ok(size)
    }

    /// One try at fetching `blob` through a circuit through `hops`, the
    /// first of them this node's own hop `my_hop`.
    async fn fetch_through_hops(
        &self,
        blob: &BlobHash,
        host: EndpointId,
        offer: &CircuitOffer,
        hops: &[Descriptor],
        my_hop: &EndpointId,
        job: (BlobClass, u64),
    ) -> Result<u64> {
        let circuit = self
            .open(blob, offer, hops)
            .await
            .inspect_err(|_| self.forget_descriptor(my_hop))?;
        self.download_over(circuit, None, blob, host, offer, job)
            .await
    }

    /// A circuit through `hops` to `offer`'s slot, on the link kept to
    /// this node's own hop, or on a link of its own.
    async fn open(
        &self,
        blob: &BlobHash,
        offer: &CircuitOffer,
        hops: &[Descriptor],
    ) -> Result<Circuit> {
        let started = tokio::time::Instant::now();
        let first = hops[0].verify(unix_now())?;
        let link = self.link_to(&first, blob).await?;
        match self.build_circuit(&link, blob, offer, hops, started).await {
            Ok((ends, closer)) => Ok(Circuit { link, closer, ends }),
            Err(e) => {
                self.keep_or_close(link, false).await;
                Err(e)
            }
        }
    }

    /// Builds the circuit through `through`, which has a machine in the
    /// middle, and the one through `without`, at once. The middle's is used
    /// if it's ready within `PREFER_MIDDLE_FOR`, else whichever is ready
    /// first after that. A middle whose circuit fails is noted
    /// (`middle_failed`); one that is only slower than the other isn't. The
    /// circuit not used is closed. Whether the middle's is the one returned.
    async fn open_preferring_middle(
        &self,
        blob: &BlobHash,
        offer: &CircuitOffer,
        through: &[Descriptor],
        without: &[Descriptor],
        middle: &Middle,
    ) -> Result<(Circuit, bool)> {
        // Both start at this node's own hop: the one without the middle may
        // have the kept link, the other dials one of its own.
        let started = tokio::time::Instant::now();
        let first = without[0].verify(unix_now())?;
        let addrs = self.own_addrs();
        let (with_link, without_link) = tokio::join!(
            self.dial_link(&first, Some(blob), addrs),
            self.link_to(&first, blob)
        );
        let (with_link, without_link) = match (with_link, without_link) {
            (Ok(a), Ok(b)) => (a, b),
            (Ok(link), Err(e)) | (Err(e), Ok(link)) => {
                self.keep_or_close(link, false).await;
                return Err(e);
            }
            (Err(e), Err(_)) => return Err(e),
        };
        let raced = race(
            Box::pin(self.build_circuit(&with_link, blob, offer, through, started)),
            Box::pin(self.build_circuit(&without_link, blob, offer, without, started)),
            PREFER_MIDDLE_FOR,
        )
        .await;
        let machine = middle.machine.fmt_short();
        if let Some(e) = &raced.middle_failed {
            debug!(%machine, "the circuit through a machine in the middle failed; without it: {e:#}");
            self.middle_failed(middle);
        }
        if let Some((ends, closer)) = raced.unused {
            ends.close().await;
            closer.close();
        }
        match raced.chosen {
            Ok(((ends, closer), true)) => {
                self.keep_or_close(without_link, false).await;
                Ok((
                    Circuit {
                        link: with_link,
                        closer,
                        ends,
                    },
                    true,
                ))
            }
            Ok(((ends, closer), false)) => {
                if raced.middle_failed.is_none() {
                    debug!(%machine, "the circuit without the machine in the middle was ready first; it carries this one");
                }
                self.keep_or_close(with_link, false).await;
                Ok((
                    Circuit {
                        link: without_link,
                        closer,
                        ends,
                    },
                    false,
                ))
            }
            Err(e) => {
                self.keep_or_close(with_link, false).await;
                self.keep_or_close(without_link, false).await;
                Err(e)
            }
        }
    }

    /// An endpoint for a circuit's first link: with a key made for this
    /// circuit, published nowhere, so the first hop learns an address
    /// that's fetching, not an account.
    async fn link_endpoint(&self) -> Result<Endpoint> {
        let relays = self.tracker.relays().clone();
        let relay = relay_mode(&self.config)?.context("relays are off")?;
        relayed_endpoint(&self.config, relay, &relays, SecretKey::generate(), vec![])
            .await
            .context("starting the endpoint for the circuit")
    }

    /// Dials `first`, a circuit's first hop, from an endpoint made for the
    /// link.
    async fn dial_link(
        &self,
        first: &CarrierInfo,
        blob: Option<&BlobHash>,
        addrs: Vec<SocketAddr>,
    ) -> Result<Link> {
        let started = tokio::time::Instant::now();
        let endpoint = self.link_endpoint().await?;
        let relays = self.tracker.relays().clone();
        let dial = osp_carrier::relay_dial(endpoint.clone(), move |url| {
            relays.permits(&TransportAddr::Relay(url.clone()))
        });
        let conn = match tokio::time::timeout(SETUP_TIMEOUT, dial(first.clone())).await {
            Ok(Ok(conn)) => conn,
            Ok(Err(e)) => {
                endpoint.close().await;
                return Err(e);
            }
            Err(_) => {
                endpoint.close().await;
                bail!("the first hop didn't answer in time");
            }
        };
        debug!(
            blob = blob.map(|b| b.fmt_short()),
            connect_ms = started.elapsed().as_millis() as u64,
            "building a circuit: connected to the first hop"
        );
        Ok(Link {
            endpoint,
            conn,
            machine: first.machine,
            to: first.endpoint,
            made: tokio::time::Instant::now(),
            addrs,
            token: Arc::new(()),
        })
    }

    /// The link for a circuit whose first hop is `first`: the one kept to
    /// this node's own hop, if it's to `first` and still good, else a new
    /// one. A kept link that's stale is closed.
    async fn link_to(&self, first: &CarrierInfo, blob: &BlobHash) -> Result<Link> {
        let addrs = self.own_addrs();
        let (kept, stale) = {
            let mut slot = self.own_link.kept.lock().unwrap();
            match slot.take() {
                Some(link) if link.to == first.endpoint && link.usable(&addrs) => {
                    *self.own_link.lent.lock().unwrap() = Arc::downgrade(&link.token);
                    self.own_link
                        .used
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    (Some(link), None)
                }
                Some(link) if !link.usable(&addrs) => (None, Some(link)),
                // Kept to another hop: left for it.
                other => {
                    *slot = other;
                    (None, None)
                }
            }
        };
        if let Some(stale) = stale {
            stale.close().await;
        }
        match kept {
            Some(link) => {
                debug!(blob = %blob.fmt_short(), "building a circuit: on the link kept to this node's own hop");
                Ok(link)
            }
            None => self.dial_link(first, Some(blob), addrs).await,
        }
    }

    /// Keeps `link` as the link to this node's own hop if none is kept and
    /// it's still good, else closes it. With `moved`, it also takes the
    /// place of one kept to another hop. A circuit's link comes back here
    /// once the circuit is ended.
    async fn keep_or_close(&self, link: Link, moved: bool) {
        let refused = {
            let mut slot = self.own_link.kept.lock().unwrap();
            let room = match slot.as_ref() {
                None => true,
                Some(kept) => moved && kept.to != link.to,
            };
            if room && self.fetches_through_carriers() && link.usable(&self.own_addrs()) {
                slot.replace(link)
            } else {
                Some(link)
            }
        };
        if let Some(link) = refused {
            link.close().await;
        }
    }

    /// Ends `circuit`, and keeps its link or closes it. Its first hop is
    /// the own hop this node's fetches went through, so the link kept moves
    /// there: one kept at start may be to the hop of the hosts it fetches
    /// from, which a circuit leaves out.
    async fn close_circuit(&self, circuit: Circuit) {
        circuit.ends.close().await;
        circuit.closer.close();
        self.keep_or_close(circuit.link, true).await;
    }

    /// This node's own addresses, as its carrier endpoint finds them: what
    /// a hop sees it at. A kept link made at others is replaced.
    fn own_addrs(&self) -> Vec<SocketAddr> {
        let mut addrs: Vec<SocketAddr> = self
            .carrier
            .as_ref()
            .map(|c| c.endpoint.addr().ip_addrs().copied().collect())
            .unwrap_or_default();
        addrs.sort();
        addrs
    }

    /// Makes the link kept to this node's own hop if there's none, and none
    /// is lent: at start, and again once it closed, its day is up or this
    /// node's addresses changed. Without carriers, the one kept goes.
    async fn keep_own_link(&self) -> Result<()> {
        if !self.fetches_through_carriers() {
            let kept = self.own_link.kept.lock().unwrap().take();
            if let Some(link) = kept {
                link.close().await;
            }
            return Ok(());
        }
        let addrs = self.own_addrs();
        let stale = {
            let mut slot = self.own_link.kept.lock().unwrap();
            match slot.as_ref() {
                Some(link) if link.usable(&addrs) => return Ok(()),
                Some(_) => slot.take(),
                None if self.own_link.lent.lock().unwrap().upgrade().is_some() => return Ok(()),
                None => None,
            }
        };
        if let Some(stale) = stale {
            stale.close().await;
        }
        let known = self.known_carriers()?;
        let first = self.own_hop(&known, None).await?.verify(unix_now())?;
        let link = self.dial_link(&first, None, addrs).await?;
        debug!(hop = %crate::machine::words(&link.machine), "a link is kept to this node's own hop");
        self.keep_or_close(link, false).await;
        Ok(())
    }

    /// Builds the circuit to `offer`'s slot through `hops` on `link`, to
    /// their first: the endpoint whose only path is the circuit, and what
    /// ends the circuit. `started` is when getting the link began.
    async fn build_circuit(
        &self,
        link: &Link,
        blob: &BlobHash,
        offer: &CircuitOffer,
        hops: &[Descriptor],
        started: tokio::time::Instant,
    ) -> Result<(Endpoint, CircuitCloser)> {
        let circuit = ViewerCircuit::build(link.conn.clone(), hops, offer.slot).await?;
        let closer = circuit.closer();
        let ends = match osp_carrier::viewer_endpoint(circuit, offer.endpoint).await {
            Ok(ends) => ends,
            Err(e) => {
                closer.close();
                return Err(e);
            }
        };
        debug!(
            blob = %blob.fmt_short(),
            hops = hops.len(),
            took_ms = started.elapsed().as_millis() as u64,
            "the circuit is ready"
        );
        Ok((ends, closer))
    }

    /// Fetches `blob` from `host` over `circuit`, then closes it. Over the
    /// connection already opened to it, if there is one.
    async fn download_over(
        &self,
        circuit: Circuit,
        connected: Option<Connected>,
        blob: &BlobHash,
        host: EndpointId,
        offer: &CircuitOffer,
        (class, declared): (BlobClass, u64),
    ) -> Result<u64> {
        let (pool, held) = match connected {
            Some(c) => (c.pool, Some(c._held)),
            None => (store::new_pool(&circuit.ends), None),
        };
        let source = Source {
            node: host,
            endpoint: offer.endpoint,
            from_direct: false,
        };
        let size = self
            .store
            .download_through(
                blob,
                Sources::one_at_a_time(vec![source]),
                class,
                declared,
                &pool,
            )
            .await;
        drop(held);
        drop(pool);
        self.close_circuit(circuit).await;
        size
    }

    /// As `fetch_over_circuit`, when this node fetches through carriers and
    /// the host offers a way, logging why not otherwise. Whether it fetched.
    pub(crate) async fn try_over_circuit(
        &self,
        blob: &BlobHash,
        host: EndpointId,
        offer: Option<&CircuitOffer>,
        class: BlobClass,
        declared: u64,
    ) -> Option<u64> {
        let offer = offer?;
        if !self.fetches_through_carriers() {
            return None;
        }
        match self
            .fetch_over_circuit(blob, host, offer, class, declared)
            .await
        {
            Ok(size) => Some(size),
            Err(e) => {
                debug!(blob = %blob.fmt_short(), "couldn't fetch through carriers; the relays instead: {e:#}");
                None
            }
        }
    }

    /// As `try_over_circuit`'s first step: the circuit to `offer`, built,
    /// or why not logged.
    async fn circuit_offered(&self, blob: &BlobHash, offer: &CircuitOffer) -> Option<Built> {
        self.circuit_to(blob, offer)
            .await
            .inspect_err(|e| debug!(blob = %blob.fmt_short(), "couldn't fetch through carriers; the relays instead: {e:#}"))
            .ok()
    }

    /// As `circuit_offered`, and then the connection to the host opened over
    /// it straight away, for the circuit built while other sources are
    /// tried: if they fail, the fetch over it doesn't wait for QUIC's
    /// handshake too (stage D's first byte, `docs/testing/carrier-lab.md`).
    /// Nothing is asked for over it. If it can't be opened now, the fetch
    /// opens it.
    async fn circuit_connected(&self, blob: &BlobHash, offer: &CircuitOffer) -> Option<Built> {
        let mut built = self.circuit_offered(blob, offer).await?;
        let pool = store::new_pool(&built.circuit.ends);
        let started = tokio::time::Instant::now();
        match pool.get_or_connect(offer.endpoint).await {
            Ok(held) => {
                debug!(
                    blob = %blob.fmt_short(),
                    connect_ms = started.elapsed().as_millis() as u64,
                    "connected to the host over the circuit, ahead of the fetch"
                );
                built.connected = Some(Connected { pool, _held: held });
            }
            Err(e) => debug!(
                blob = %blob.fmt_short(),
                "couldn't connect to the host over the circuit ahead; the fetch will try: {e}"
            ),
        }
        Some(built)
    }

    /// As `try_over_circuit`'s second step: the fetch over `built`.
    async fn fetch_offered(
        &self,
        built: Built,
        blob: &BlobHash,
        host: EndpointId,
        offer: &CircuitOffer,
        job: (BlobClass, u64),
    ) -> Option<u64> {
        self.fetch_over_built(built, blob, host, offer, job)
            .await
            .inspect_err(|e| debug!(blob = %blob.fmt_short(), "couldn't fetch through carriers; the relays instead: {e:#}"))
            .ok()
    }

    /// Downloads `blob` from `sources`, as the store does, but reaches the
    /// nodes in `offers` through carriers rather than their relayed media
    /// endpoints when this node fetches through carriers (ADR 0043): the
    /// other sources asked at once first, and any reached directly, then
    /// each offer's circuit, then all of `sources` as before, the relays
    /// included. A node's direct endpoint is never held back. With `via`, a viewing endpoint's pool, the
    /// sources are reached only through it (ADR 0024).
    ///
    /// The first offer's circuit is built while the other sources are
    /// tried, and the connection to the host opened over it, so a try that
    /// fails doesn't then wait for either: stage D's first byte
    /// (`docs/testing/carrier-lab.md`). Nothing is asked for over it unless
    /// the try fails. If it succeeds, the circuit and its connection are
    /// closed, or their build dropped.
    pub(crate) async fn download_offered(
        &self,
        blob: &BlobHash,
        sources: Sources,
        offers: Offers,
        (class, declared): (BlobClass, u64),
        via: Option<&ConnectionPool>,
    ) -> Result<u64> {
        if offers.is_empty() || !self.fetches_through_carriers() {
            return self
                .store
                .download_with(blob, sources, class, declared, via)
                .await;
        }
        let job = (class, declared);
        let offered = |s: &Source| !s.from_direct && offers.iter().any(|(node, _)| *node == s.node);
        // Those asked one at a time are the ones to spare, such as the
        // author's: they wait for the circuits unless reached directly.
        let mut others = sources.clone();
        others.at_once.retain(|s| !offered(s));
        others.in_turn.retain(|s| s.from_direct);
        let mut first_built = None;
        if !others.is_empty() {
            let (_, first) = &offers[0];
            let (tried, built) = try_while_building(
                self.store.download_with(blob, others, class, declared, via),
                // Boxed: a circuit's build, with a middle's beside it, is large.
                Box::pin(self.circuit_connected(blob, first)),
            )
            .await;
            match tried {
                Ok(size) => {
                    // Not needed: closed if ready, else its build dropped.
                    if let Some(Some(b)) = built {
                        drop(b.connected);
                        self.close_circuit(b.circuit).await;
                    }
                    return Ok(size);
                }
                Err(e) => {
                    debug!(blob = %blob.fmt_short(), "the other sources failed; through carriers next: {e:#}");
                    first_built = built;
                }
            }
        }
        for (i, (node, offer)) in offers.iter().enumerate() {
            // Built while the other sources were tried: if that took long,
            // it may have gone stale, so a fetch over it that fails is tried
            // once more over a new one. One whose build failed isn't.
            if let Some(built) = first_built.take().filter(|_| i == 0) {
                let Some(built) = built else { continue };
                if let Some(size) = self.fetch_offered(built, blob, *node, offer, job).await {
                    return Ok(size);
                }
            }
            if let Some(built) = self.circuit_offered(blob, offer).await
                && let Some(size) = self.fetch_offered(built, blob, *node, offer, job).await
            {
                return Ok(size);
            }
        }
        self.store
            .download_with(blob, sources, class, declared, via)
            .await
    }
}

/// Keeps a link open to this node's own hop while it fetches through
/// carriers (`OwnLink`), looking every `KEEP_LINK_CHECK`.
pub(crate) async fn keep_own_link(core: Arc<Core>) {
    if let Some(tor) = &core.tor {
        tor.ready().await;
    }
    loop {
        if let Err(e) = core.keep_own_link().await {
            debug!("no link kept to this node's own hop for now: {e:#}");
        }
        tokio::time::sleep(KEEP_LINK_CHECK).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A build that takes `secs` and comes out as `ok`, named `name`.
    async fn build(name: &'static str, secs: f64, ok: bool) -> Result<&'static str> {
        tokio::time::sleep(Duration::from_secs_f64(secs)).await;
        match ok {
            true => Ok(name),
            false => Err(anyhow!("{name} failed")),
        }
    }

    async fn raced(with: (f64, bool), without: (f64, bool)) -> (Raced<&'static str>, f64) {
        let started = tokio::time::Instant::now();
        let r = race(
            build("middle", with.0, with.1),
            build("two", without.0, without.1),
            PREFER_MIDDLE_FOR,
        )
        .await;
        // To a tenth of a second: timers are rounded to milliseconds.
        let took = (started.elapsed().as_secs_f64() * 10.0).round() / 10.0;
        (r, took)
    }

    #[tokio::test(start_paused = true)]
    async fn a_middle_ready_in_time_is_used_even_after_the_other() {
        let (r, took) = raced((4.5, true), (1.0, true)).await;
        assert_eq!(r.chosen.unwrap(), ("middle", true));
        assert_eq!(r.unused, Some("two"), "built, so it's closed");
        assert!(r.middle_failed.is_none());
        assert_eq!(took, 4.5);
    }

    #[tokio::test(start_paused = true)]
    async fn a_middle_ready_first_doesnt_wait_for_the_other() {
        let (r, took) = raced((2.0, true), (4.0, true)).await;
        assert_eq!(r.chosen.unwrap(), ("middle", true));
        assert_eq!(r.unused, None, "still building, so dropped");
        assert_eq!(took, 2.0);
    }

    #[tokio::test(start_paused = true)]
    async fn a_slow_middle_loses_to_the_other_without_failing() {
        // Stage D: a middle that never answered held the fetch 22.8 s.
        let (r, took) = raced((20.0, false), (4.2, true)).await;
        assert_eq!(r.chosen.unwrap(), ("two", false));
        assert!(r.middle_failed.is_none(), "only slower: not failed");
        assert_eq!(took, 5.0, "the middle's 5 s, then the one ready");
        let (r, took) = raced((30.0, true), (7.0, true)).await;
        assert_eq!(r.chosen.unwrap(), ("two", false));
        assert_eq!(took, 7.0, "after the 5 s, the first ready");
    }

    #[tokio::test(start_paused = true)]
    async fn a_middle_that_fails_is_noted_and_the_other_carries() {
        // Stage D: a middle answered `Gone` after 5.4 s.
        let (r, took) = raced((1.0, false), (4.2, true)).await;
        assert_eq!(r.chosen.unwrap(), ("two", false));
        assert!(r.middle_failed.is_some());
        assert_eq!(took, 4.2, "no wait for the 5 s once the middle failed");
    }

    #[tokio::test(start_paused = true)]
    async fn the_middle_carries_when_the_other_fails_even_late() {
        let (r, took) = raced((9.0, true), (1.0, false)).await;
        assert_eq!(r.chosen.unwrap(), ("middle", true));
        assert_eq!(took, 9.0);
        let (r, _) = raced((2.0, false), (3.0, false)).await;
        assert!(r.chosen.unwrap_err().to_string().contains("two failed"));
        assert!(r.middle_failed.is_some());
    }

    /// A try that takes `secs` and comes out as `ok`, with the circuit
    /// built in `build_secs` beside it: how they came out, and the time.
    async fn tried(secs: f64, ok: bool, build_secs: f64) -> (bool, Option<&'static str>, f64) {
        let started = tokio::time::Instant::now();
        let (r, built) = try_while_building(build("direct", secs, ok), async move {
            tokio::time::sleep(Duration::from_secs_f64(build_secs)).await;
            "circuit"
        })
        .await;
        let took = (started.elapsed().as_secs_f64() * 10.0).round() / 10.0;
        (r.is_ok(), built, took)
    }

    #[tokio::test(start_paused = true)]
    async fn a_direct_try_that_fails_leaves_the_circuit_built_beside_it() {
        // Stage D: the direct try failed after 1 s, then the circuit took
        // 4 s more. Built beside it, the 4 s are all it costs.
        assert_eq!(tried(1.0, false, 4.0).await, (false, Some("circuit"), 4.0));
        assert_eq!(tried(6.0, false, 4.0).await, (false, Some("circuit"), 6.0));
    }

    #[tokio::test(start_paused = true)]
    async fn a_direct_try_that_succeeds_doesnt_wait_for_the_circuit() {
        assert_eq!(tried(1.0, true, 4.0).await, (true, None, 1.0), "dropped");
        assert_eq!(
            tried(30.0, true, 4.0).await,
            (true, Some("circuit"), 30.0),
            "built, so handed back to be closed"
        );
    }

    #[test]
    fn a_files_hosts_are_asked_for_offers_once_at_a_time() {
        let asked = OffersAsked::default();
        let blob = BlobHash::from_bytes([1; 32]);
        let first = asked.start(blob).expect("not asked yet");
        assert!(asked.start(blob).is_none(), "asked twice at once");
        assert!(asked.start(BlobHash::from_bytes([2; 32])).is_some());
        drop(first);
        assert!(asked.start(blob).is_some(), "not asked again once done");
    }

    #[test]
    fn an_offer_whose_descriptor_doesnt_check_out_is_refused() {
        let offer = CircuitOffer {
            carrier: "not-a-descriptor!".into(),
            slot: [0; 16],
            endpoint: SecretKey::generate().public(),
        };
        assert!(offered_hop(&offer, unix_now()).is_err());
        let offer = CircuitOffer {
            carrier: data_encoding::BASE64URL_NOPAD.encode(&[7; 40]),
            ..offer
        };
        assert!(offered_hop(&offer, unix_now()).is_err());
    }
}
