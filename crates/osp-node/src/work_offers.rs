//! Work offers (ADR 0043 stage B part 4d-2, as the maintainer settled it
//! 2026-11-20): how a machine new to a viewer gets a first chance through a
//! machine the viewer already trusts.
//!
//! Say Mo's new machine had the middle of one of Jimmy's fetches, and the
//! file passed its hash check: Mo's first job for Jimmy's node. Jimmy's node
//! remembers the work as it remembers any (`db::carriers`: standing, and
//! that Mo's machine ever worked for it), and apart from that makes Mo's
//! machine one it may offer on: to at most 2 who ask, ever, the newest
//! first, for 30 days from Mo's last finished job for it, while Mo's
//! machine is in its table, and until a circuit with Mo's machine in the
//! middle fails. Later work keeps the offer going, never adds handouts.
//! Alice's node holds Jimmy's machine at standing,
//! and after one of her fetches through hops it has room for a newcomer: a
//! probation place free, no fresh machine waiting, no offer held. So it asks
//! Jimmy's carrier "know a machine that could work for me?", from a key made
//! for the ask, as her circuits do, so Jimmy's node never learns who asked.
//! It names Mo's machine, with the descriptor its table has now. Alice's
//! node keeps the offer up to 30 days, and Mo's machine has her once-a-day
//! newcomer turn in the middle when no fresh machine is waiting. After that
//! one job it's like any machine that finished one: probation, then standing
//! by jobs on 3 days in 30. An offer is never standing.
//!
//! Alice's node asks only when it has room, one machine at standing at a
//! time, the one asked longest ago, and each at most once a day, so one
//! machine isn't always the one that recommends. Nobody pushes offers: a
//! carrier doesn't know whom it carried for.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result, anyhow};
use iroh::{EndpointAddr, EndpointId, SecretKey, TransportAddr, endpoint::Connection};
use osp_proto::{
    framing::{read_frame, read_frame_max, write_frame},
    social::{MAX_REQUEST_LEN, WORK_OFFERS_ALPN, WorkOffer, WorkOffersRequest, WorkOffersResponse},
};
use tokio::sync::{Notify, Semaphore};
use tracing::debug;

use crate::{
    carrier::{relay_mode, relayed_endpoint},
    db::{DAY_MS, Db, SUGGESTION_DAYS, day_of, now_ms},
    hops::unix_now,
    machine::{descriptor_from_text, descriptor_text},
    meet::Acquaintances,
    node::Core,
    viewing::in_own_order,
};

/// Asks answered at once, from everyone together: askers come with new keys.
const MAX_ANSWERS: usize = 16;
/// Longest one ask may take, at either end.
const ASK_WAIT: Duration = Duration::from_secs(10);
/// An offer's descriptor is asked for afresh once it has less than this
/// left, when the table doesn't have a newer one.
const RENEW_BEFORE: u64 = 60 * 60;
/// The longest an offered descriptor's text may be: unpadded base64 of the
/// most bytes a descriptor has.
const MAX_DESCRIPTOR_TEXT: usize = osp_carrier::MAX_DESCRIPTOR_BYTES.div_ceil(3) * 4;

/// The asking side: what wakes it, and the day each machine was last asked.
#[derive(Default)]
pub(crate) struct WorkAsks {
    wake: Notify,
    asked: Mutex<HashMap<EndpointId, i64>>,
}

/// The carrier endpoint's answers to [`WORK_OFFERS_ALPN`], from the tips
/// in `db`, for machines `table` names as carriers now.
pub(crate) fn answerer(db: Arc<Db>, table: Arc<Acquaintances>) -> osp_carrier::Others {
    let answers = Arc::new(Semaphore::new(MAX_ANSWERS));
    Arc::new(move |conn: Connection| {
        if conn.alpn() != WORK_OFFERS_ALPN {
            conn.close(0u32.into(), b"not a carrier link");
            return;
        }
        let Ok(permit) = answers.clone().try_acquire_owned() else {
            conn.close(0u32.into(), b"busy");
            return;
        };
        let db = db.clone();
        let table = table.clone();
        tokio::spawn(async move {
            let _permit = permit;
            if let Err(e) = tokio::time::timeout(ASK_WAIT, answer(&db, &table, &conn)).await {
                debug!("an ask for work took too long: {e}");
            }
        });
    })
}

/// One ask for work, on its own connection.
async fn answer(db: &Db, table: &Acquaintances, conn: &Connection) -> Result<()> {
    let (mut send, mut recv) = conn.accept_bi().await?;
    let _: WorkOffersRequest = read_frame_max(&mut recv, MAX_REQUEST_LEN).await?;
    // Only a machine with a carrier that checks out now, by its record in
    // the table: one gone from it has nothing current to offer.
    let carriers: HashMap<EndpointId, String> = table
        .carriers(unix_now())
        .into_iter()
        .map(|c| (c.machine, descriptor_text(&c.descriptor)))
        .collect();
    let offers = match db.hand_tip(now_ms(), &|m| carriers.contains_key(m))? {
        Some(machine) => {
            debug!(machine = %machine.fmt_short(), "offered a machine for work to one who asked");
            vec![WorkOffer {
                machine,
                carrier: carriers[&machine].clone(),
            }]
        }
        None => Vec::new(),
    };
    write_frame(&mut send, &WorkOffersResponse { offers }).await?;
    send.finish()?;
    conn.closed().await;
    Ok(())
}

/// Asks for work offers when woken, one ask at a time.
pub(crate) async fn ask_loop(core: Arc<Core>) {
    loop {
        core.work_asks.wake.notified().await;
        if let Err(e) = core.ask_for_work().await {
            debug!("asking for work offers: {e:#}");
        }
    }
}

impl Core {
    /// After a fetch through hops: asks for a work offer if there's room.
    pub(crate) fn may_ask_for_work(&self) {
        self.work_asks.wake.notify_one();
    }

    /// If this node has room for a newcomer, asks the machine at standing
    /// asked longest ago, not today, for one, then keeps the offers' descriptors
    /// fresh.
    pub(crate) async fn ask_for_work(&self) -> Result<()> {
        let known = self.known_carriers()?;
        let worked = |m: &EndpointId| self.db.carrier_worked(m).unwrap_or(true);
        if known.has_room_for_an_offer(&worked)
            && let Some(machine) = self.next_to_ask(&known.standing)
        {
            let offers = match self.ask_one_for_work(machine).await {
                Ok(offers) => offers,
                Err(e) => {
                    debug!(machine = %machine.fmt_short(), "asking for work offers: {e:#}");
                    Vec::new()
                }
            };
            let me = self.machine().map(|m| m.id()).ok();
            let offer = offers.into_iter().find(|o| {
                Some(o.machine) != me
                    && o.carrier.len() <= MAX_DESCRIPTOR_TEXT
                    && known.may_take_offer(&o.machine, worked(&o.machine))
            });
            if let Some(o) = offer {
                let until = now_ms() + SUGGESTION_DAYS * DAY_MS;
                if self
                    .db
                    .add_suggestion(&o.machine, &o.carrier, &machine, until)?
                {
                    debug!(machine = %o.machine.fmt_short(), from = %machine.fmt_short(), "took a work offer");
                }
            }
        }
        self.renew_offers().await
    }

    /// The machine at standing to ask: one never asked first, else the one
    /// asked longest ago, never one asked today. Noted as asked.
    fn next_to_ask(&self, standing: &std::collections::HashSet<EndpointId>) -> Option<EndpointId> {
        let today = day_of(now_ms());
        let seeds: Vec<EndpointId> = self
            .config
            .carrier
            .known
            .iter()
            .map(|s| s.machine)
            .collect();
        let me = self.machine().map(|m| m.id()).ok();
        let mut asked = self
            .work_asks
            .asked
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        asked.retain(|m, _| standing.contains(m));
        let mut open: Vec<EndpointId> = standing
            .iter()
            .filter(|m| !seeds.contains(m) && Some(**m) != me && asked.get(*m) != Some(&today))
            .copied()
            .collect();
        in_own_order(&mut open);
        open.sort_by_key(|m| asked.get(m).copied().unwrap_or(i64::MIN));
        let machine = open.into_iter().next()?;
        asked.insert(machine, today);
        Some(machine)
    }

    /// One ask for work to `machine`'s carrier, from a key made for it.
    async fn ask_one_for_work(&self, machine: EndpointId) -> Result<Vec<WorkOffer>> {
        let descriptor = self.seed_descriptor(machine, true).await?;
        let info = descriptor.verify(unix_now())?;
        let relay: iroh::RelayUrl = info.relay.parse().context("its descriptor's relay")?;
        let relays = self.tracker.relays().clone();
        anyhow::ensure!(
            relays.permits(&TransportAddr::Relay(relay.clone())),
            "a relay this node doesn't use: {relay}"
        );
        let mode = relay_mode(&self.config)?.context("relays are off")?;
        // Published nowhere, so the one asked learns an address that asks,
        // not who.
        let endpoint =
            relayed_endpoint(&self.config, mode, &relays, SecretKey::generate(), vec![]).await?;
        let asked = tokio::time::timeout(ASK_WAIT, async {
            let addr = EndpointAddr::new(info.endpoint).with_relay_url(relay);
            let conn = endpoint.connect(addr, WORK_OFFERS_ALPN).await?;
            let (mut send, mut recv) = conn.open_bi().await?;
            write_frame(&mut send, &WorkOffersRequest {}).await?;
            send.finish()?;
            let resp: WorkOffersResponse = read_frame(&mut recv).await?;
            conn.close(0u32.into(), b"done");
            anyhow::Ok(resp.offers)
        })
        .await
        .map_err(|_| anyhow!("the machine asked for work didn't answer in time"));
        endpoint.close().await;
        let offers = asked??;
        debug!(machine = %machine.fmt_short(), offers = offers.len(), "asked a machine at standing for work offers");
        Ok(offers)
    }

    /// Asks each offered machine not in the table for its descriptor, when
    /// the one held has run out or soon will.
    async fn renew_offers(&self) -> Result<()> {
        let now = unix_now();
        let table: Vec<EndpointId> = match self.machine() {
            Ok(machine) => machine
                .acquaintances
                .carriers(now)
                .iter()
                .map(|c| c.machine)
                .collect(),
            Err(_) => return Ok(()),
        };
        for (machine, text) in self.db.suggestions(now_ms())? {
            let fresh_enough = descriptor_from_text(&text, &machine, now + RENEW_BEFORE).is_some();
            if fresh_enough || table.contains(&machine) {
                continue;
            }
            match tokio::time::timeout(ASK_WAIT, self.seed_descriptor(machine, false)).await {
                Ok(Ok(descriptor)) => {
                    let text = descriptor_text(&descriptor);
                    self.db.renew_suggestion(&machine, &text)?;
                }
                Ok(Err(e)) => {
                    debug!(machine = %machine.fmt_short(), "an offered machine's descriptor: {e:#}")
                }
                Err(_) => {
                    debug!(machine = %machine.fmt_short(), "an offered machine didn't answer in time")
                }
            }
        }
        Ok(())
    }
}
