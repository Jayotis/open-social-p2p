//! The two ends of a circuit: the host's link to its hop, and the viewer's
//! circuit through the hops it picked.
//!
//! Both carry the ends' own packets, already under their own encryption.
//! The circuit transport puts iroh's QUIC on them.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail, ensure};
use bytes::Bytes;
use iroh::endpoint::Connection;
use tokio::{
    sync::mpsc::{self, error::TrySendError},
    task::{AbortHandle, JoinSet},
    time::timeout,
};
use tracing::debug;

use crate::{
    CircuitId, Descriptor, DestroyReason, Direction, Frame, MAX_HOPS, Onion, Relay, Setup, Slot,
    service::{SETUP_TIMEOUT, request_slot, wait_for_room},
};

/// Packets waiting to be read, at either end. More are dropped, as a full
/// socket buffer drops them.
const INBOX: usize = 1024;

/// Packets dropped at this node's circuit ends because the inbox was full:
/// the end's QUIC wasn't reading them as fast as they came. Since the
/// process started, for every end at once. Stage D's sixth run broke on
/// packets lost where nothing counted.
static VIEWER_INBOX_FULL: AtomicU64 = AtomicU64::new(0);
static HOST_INBOX_FULL: AtomicU64 = AtomicU64::new(0);
/// Sends at this node's circuit ends that found less room in the link's
/// datagram queue than the frame needs, as the carrier's `send_queue_full`
/// counts them: QUIC then discards the oldest waiting, without a word.
static VIEWER_SEND_QUEUE_FULL: AtomicU64 = AtomicU64::new(0);
static HOST_SEND_QUEUE_FULL: AtomicU64 = AtomicU64::new(0);

/// What went missing at this node's circuit ends since the process
/// started, for every end at once: packets dropped with the inbox full, at
/// the viewer ends of its circuits and at its host links; and sends that
/// found the link's queue full, at each.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InboxDrops {
    pub viewer: u64,
    pub host: u64,
    pub viewer_send_queue_full: u64,
    pub host_send_queue_full: u64,
}

pub fn inbox_drops() -> InboxDrops {
    InboxDrops {
        viewer: VIEWER_INBOX_FULL.load(Ordering::Relaxed),
        host: HOST_INBOX_FULL.load(Ordering::Relaxed),
        viewer_send_queue_full: VIEWER_SEND_QUEUE_FULL.load(Ordering::Relaxed),
        host_send_queue_full: HOST_SEND_QUEUE_FULL.load(Ordering::Relaxed),
    }
}

/// Sends `frame` on `conn`, counting in `full` a send that finds less room
/// in its datagram queue than the frame needs.
fn send_counted(
    conn: &Connection,
    frame: Bytes,
    full: &AtomicU64,
) -> Result<(), iroh::endpoint::SendDatagramError> {
    if conn.datagram_send_buffer_space() < frame.len() {
        full.fetch_add(1, Ordering::Relaxed);
    }
    conn.send_datagram(frame)
}

/// What comes to a host over its link to its hop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostEvent {
    /// A viewer's circuit joined, through one of this link's slots.
    Opened(CircuitId),
    Packet(CircuitId, Vec<u8>),
    Closed(CircuitId),
}

/// A host's link to its hop Q: the slots Q gave it, and the circuits Q
/// joined to it.
pub struct HostLink {
    sender: HostSender,
    events: mpsc::Receiver<HostEvent>,
}

/// The sending half of a [`HostLink`], with its slot. The link lasts while
/// it's held.
#[derive(Clone)]
pub struct HostSender {
    conn: Connection,
    slot: Arc<Mutex<Slot>>,
    _tasks: Arc<JoinSet<()>>,
}

impl HostLink {
    /// Asks the carrier at the other end of `conn` for a slot, and keeps
    /// asking for a new one before each runs out.
    pub async fn open(conn: Connection) -> Result<Self> {
        wait_for_room(&conn, SETUP_TIMEOUT).await?;
        let (slot, lasts) = request_slot(&conn).await.context("no host slot")?;
        let slot = Arc::new(Mutex::new(slot));
        let mut tasks = JoinSet::new();
        let (tx, events) = mpsc::channel(INBOX);
        let reader = conn.clone();
        tasks.spawn(async move {
            while let Ok(bytes) = reader.read_datagram().await {
                let ev = match Frame::decode(&bytes) {
                    Ok(Frame::Created { circuit }) => HostEvent::Opened(circuit),
                    Ok(Frame::Data {
                        circuit,
                        direction: Direction::ToHost,
                        payload,
                        ..
                    }) => HostEvent::Packet(circuit, payload),
                    Ok(Frame::Destroy { circuit, .. }) => HostEvent::Closed(circuit),
                    _ => continue,
                };
                match tx.try_send(ev) {
                    Ok(()) => {}
                    Err(TrySendError::Full(_)) => {
                        HOST_INBOX_FULL.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(TrySendError::Closed(_)) => return,
                }
            }
        });
        let refresh = conn.clone();
        let held = slot.clone();
        tasks.spawn(async move {
            let mut wait = lasts / 3;
            loop {
                tokio::time::sleep(wait.max(Duration::from_secs(1))).await;
                match request_slot(&refresh).await {
                    Ok((s, lasts)) => {
                        *held.lock().expect("slot lock") = s;
                        wait = lasts / 3;
                    }
                    Err(e) => {
                        debug!("couldn't renew the host slot: {e:#}");
                        if refresh.close_reason().is_some() {
                            return;
                        }
                        wait = Duration::from_secs(10);
                    }
                }
            }
        });
        Ok(Self {
            sender: HostSender {
                conn,
                slot,
                _tasks: Arc::new(tasks),
            },
            events,
        })
    }

    pub fn slot(&self) -> Slot {
        self.sender.slot()
    }

    pub fn send(&self, circuit: CircuitId, packet: Vec<u8>) -> Result<()> {
        self.sender.send(circuit, packet)
    }

    pub fn close(&self, circuit: CircuitId) {
        self.sender.close(circuit)
    }

    pub async fn recv(&mut self) -> Option<HostEvent> {
        self.events.recv().await
    }

    /// Another sending half, to read the slot from after the link is handed
    /// to [`host_endpoint`](crate::host_endpoint).
    pub fn sender(&self) -> HostSender {
        self.sender.clone()
    }

    /// The sending half, and what comes over the link.
    pub fn split(self) -> (HostSender, mpsc::Receiver<HostEvent>) {
        (self.sender, self.events)
    }
}

impl HostSender {
    /// The slot to hand a viewer now, with Q's descriptor.
    pub fn slot(&self) -> Slot {
        *self.slot.lock().expect("slot lock")
    }

    /// Sends a packet back to the viewer on `circuit`.
    pub fn send(&self, circuit: CircuitId, packet: Vec<u8>) -> Result<()> {
        let frame = Frame::Data {
            circuit,
            direction: Direction::ToViewer,
            pn: 0,
            payload: packet,
        };
        send_counted(
            &self.conn,
            Bytes::from(frame.encode()),
            &HOST_SEND_QUEUE_FULL,
        )?;
        Ok(())
    }

    pub fn close(&self, circuit: CircuitId) {
        let frame = Frame::Destroy {
            circuit,
            reason: DestroyReason::Closed,
        };
        let _ = self.conn.send_datagram(Bytes::from(frame.encode()));
    }

    /// Waits until the link to the hop has closed.
    pub async fn closed(&self) {
        self.conn.closed().await;
    }

    /// Whether the link to the hop has closed.
    pub fn is_closed(&self) -> bool {
        self.conn.close_reason().is_some()
    }

    /// Ends the link to the hop, and with it every circuit on it.
    pub fn end(&self) {
        self.conn.close(0u32.into(), b"done");
    }
}

/// A viewer's circuit, built through the hops it picked to the host's slot.
pub struct ViewerCircuit {
    sender: ViewerSender,
    packets: mpsc::Receiver<Vec<u8>>,
}

/// The sending half of a [`ViewerCircuit`]. The circuit lasts while it's
/// held.
pub struct ViewerSender {
    conn: Connection,
    id: CircuitId,
    onion: Arc<Mutex<Onion>>,
    tasks: Arc<Mutex<Vec<AbortHandle>>>,
    _tasks: JoinSet<()>,
}

/// Ends a [`ViewerCircuit`] from outside it, wherever it went: tells the
/// first hop, and stops reading the link, so the link can carry the next
/// circuit. Say Jimmy's node keeps its link to its own hop open between
/// fetches: each circuit's reader must be gone before the next is built on
/// the link, or it would take the next one's frames.
#[derive(Clone)]
pub struct CircuitCloser {
    conn: Connection,
    id: CircuitId,
    tasks: Arc<Mutex<Vec<AbortHandle>>>,
}

impl CircuitCloser {
    pub fn close(&self) {
        let frame = Frame::Destroy {
            circuit: self.id,
            reason: DestroyReason::Closed,
        };
        let _ = self.conn.send_datagram(Bytes::from(frame.encode()));
        for task in self.tasks.lock().expect("tasks lock").drain(..) {
            task.abort();
        }
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl ViewerCircuit {
    /// Builds a circuit over `conn`, a link this viewer opened to the first
    /// hop, through `hops` in order (the last is the host's hop), and joins
    /// it to the host connection holding `slot`. Each hop gets its setup in
    /// turn, so no hop is handed the path.
    ///
    /// One circuit to a link at a time: it reads every datagram on `conn`
    /// until it's dropped or [`CircuitCloser::close`]d.
    pub async fn build(conn: Connection, hops: &[Descriptor], slot: Slot) -> Result<Self> {
        ensure!(
            !hops.is_empty() && hops.len() <= MAX_HOPS,
            "a circuit has 1 to {MAX_HOPS} hops"
        );
        let now = unix_now();
        let infos: Vec<_> = hops.iter().map(|d| d.verify(now)).collect::<Result<_>>()?;
        ensure!(
            infos[0].endpoint == conn.remote_id(),
            "the link isn't to the first hop"
        );
        let started = tokio::time::Instant::now();
        let step = |what: &str| {
            debug!(
                ms = started.elapsed().as_millis() as u64,
                "building a circuit: {what}"
            )
        };
        wait_for_room(&conn, SETUP_TIMEOUT).await?;
        step("the link has room");
        for pair in infos.windows(2) {
            ensure!(
                pair[0].machine != pair[1].machine,
                "the same carrier twice in a row"
            );
        }

        let (frames_tx, mut frames) = mpsc::channel(INBOX);
        let mut tasks = JoinSet::new();
        let aborts: Arc<Mutex<Vec<AbortHandle>>> = Arc::default();
        let reader = conn.clone();
        let reading = tasks.spawn(async move {
            while let Ok(bytes) = reader.read_datagram().await {
                if let Ok(frame) = Frame::decode(&bytes)
                    && frames_tx.send(frame).await.is_err()
                {
                    return;
                }
            }
        });
        aborts.lock().expect("tasks lock").push(reading);

        let id = {
            let mut b = [0; 4];
            getrandom::fill(&mut b).map_err(|e| anyhow!("no random numbers: {e}"))?;
            u32::from_be_bytes(b) >> 1
        };
        let send = |frame: Frame| -> Result<()> {
            conn.send_datagram(Bytes::from(frame.encode()))
                .map_err(|e| anyhow!("couldn't send: {e}"))
        };

        let mut onion = Onion::new();
        let (setup, layer) = Setup::begin(&infos[0])?;
        send(Frame::Create {
            circuit: id,
            setup,
            backward_start: 0,
        })?;
        loop {
            match next_frame(&mut frames, id).await? {
                Frame::Created { .. } => break,
                Frame::Busy { .. } => bail!("the first hop is busy"),
                Frame::Destroy { reason, .. } => bail!("the first hop refused: {reason:?}"),
                _ => continue,
            }
        }
        step("the first hop created it");
        onion.push(layer);

        let ask = |onion: &mut Onion, relay: &Relay| -> Result<()> {
            let (pn, payload) = onion.wrap(relay)?;
            send(Frame::Data {
                circuit: id,
                direction: Direction::ToHost,
                pn,
                payload,
            })
        };
        for (info, descriptor) in infos[1..].iter().zip(&hops[1..]) {
            let (setup, layer) = Setup::begin(info)?;
            ask(
                &mut onion,
                &Relay::Extend {
                    setup,
                    descriptor: descriptor.clone(),
                },
            )?;
            match answer(&mut frames, id, &mut onion).await? {
                Relay::Extended => {
                    onion.push(layer);
                    step("extended to the next hop");
                }
                Relay::Busy => bail!("a hop couldn't extend: busy"),
                other => bail!("an unexpected answer to EXTEND: {other:?}"),
            }
        }
        ask(&mut onion, &Relay::ConnectHost { slot })?;
        match answer(&mut frames, id, &mut onion).await? {
            Relay::Connected => {}
            other => bail!("an unexpected answer to CONNECT_HOST: {other:?}"),
        }
        step("joined to the host's slot");

        let onion = Arc::new(Mutex::new(onion));
        let (packets_tx, packets) = mpsc::channel(INBOX);
        let theirs = onion.clone();
        let unwrapping = tasks.spawn(async move {
            while let Some(frame) = frames.recv().await {
                match frame {
                    Frame::Data {
                        circuit,
                        direction: Direction::ToViewer,
                        pn,
                        payload,
                    } if circuit == id => {
                        let relay = theirs.lock().expect("onion lock").unwrap(pn, payload);
                        if let Ok(Relay::Data(packet)) = relay
                            && let Err(TrySendError::Full(_)) = packets_tx.try_send(packet)
                        {
                            VIEWER_INBOX_FULL.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    Frame::Destroy { circuit, .. } if circuit == id => return,
                    _ => {}
                }
            }
        });
        aborts.lock().expect("tasks lock").push(unwrapping);
        Ok(Self {
            sender: ViewerSender {
                conn,
                id,
                onion,
                tasks: aborts,
                _tasks: tasks,
            },
            packets,
        })
    }

    /// Sends a packet to the host.
    pub fn send(&self, packet: Vec<u8>) -> Result<()> {
        self.sender.send(packet)
    }

    /// The next packet from the host; `None` once the circuit has ended.
    pub async fn recv(&mut self) -> Option<Vec<u8>> {
        self.packets.recv().await
    }

    pub fn close(&self) {
        self.sender.close()
    }

    /// What ends this circuit later, once it has gone into an endpoint.
    pub fn closer(&self) -> CircuitCloser {
        CircuitCloser {
            conn: self.sender.conn.clone(),
            id: self.sender.id,
            tasks: self.sender.tasks.clone(),
        }
    }

    /// The sending half, and the packets from the host.
    pub fn split(self) -> (ViewerSender, mpsc::Receiver<Vec<u8>>) {
        (self.sender, self.packets)
    }
}

impl ViewerSender {
    /// Sends a packet to the host.
    pub fn send(&self, packet: Vec<u8>) -> Result<()> {
        let (pn, payload) = self
            .onion
            .lock()
            .expect("onion lock")
            .wrap(&Relay::Data(packet))?;
        let frame = Frame::Data {
            circuit: self.id,
            direction: Direction::ToHost,
            pn,
            payload,
        };
        send_counted(
            &self.conn,
            Bytes::from(frame.encode()),
            &VIEWER_SEND_QUEUE_FULL,
        )?;
        Ok(())
    }

    pub fn close(&self) {
        let frame = Frame::Destroy {
            circuit: self.id,
            reason: DestroyReason::Closed,
        };
        let _ = self.conn.send_datagram(Bytes::from(frame.encode()));
    }
}

async fn next_frame(frames: &mut mpsc::Receiver<Frame>, id: CircuitId) -> Result<Frame> {
    loop {
        let frame = timeout(SETUP_TIMEOUT, frames.recv())
            .await
            .map_err(|_| anyhow!("a hop didn't answer in time"))?
            .ok_or_else(|| anyhow!("the link closed"))?;
        if frame.circuit() == id {
            return Ok(frame);
        }
    }
}

/// The last hop's answer to a command, under every layer.
async fn answer(
    frames: &mut mpsc::Receiver<Frame>,
    id: CircuitId,
    onion: &mut Onion,
) -> Result<Relay> {
    loop {
        match next_frame(frames, id).await? {
            Frame::Data {
                direction: Direction::ToViewer,
                pn,
                payload,
                ..
            } => return onion.unwrap(pn, payload),
            Frame::Destroy { reason, .. } => bail!("the circuit ended: {reason:?}"),
            _ => continue,
        }
    }
}
