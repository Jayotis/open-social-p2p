//! "How are we connected?" (ADR 0037), on the owner's node.
//!
//! Kim taps Jimmy and asks. Her node puts together the paths it knows:
//!
//! - Her own world placing Jimmy himself: she follows or knows him, or her
//!   node holds a reshare of his by someone she follows.
//! - Through Jimmy's side, where it meets her world (ADR 0029), from the
//!   same code that lets replies in. If Jimmy replied, the paths kept from
//!   that meeting. If not, her node asks Jimmy's node `WhoVouched` once,
//!   from a throwaway key over Tor (relay-only without Tor, as viewing
//!   endpoints are, ADR 0024), so his node learns someone asked, not who.
//!
//! The tap is the only thing that asks. Drawing the map, selecting Jimmy,
//! opening his profile, and anything the node does in the background never
//! do (ADR 0033). Her circle's nodes aren't asked either: their answers come
//! only from a reply's meeting.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use iroh::{Endpoint, EndpointAddr, PublicKey, SecretKey};
use osp_proto::{
    Payload, SOCIAL_ALPN,
    connected::{Connection, Named, Path, SideFrom},
    social::{CarriedVouch, MAX_VOUCHES_ANSWERED, SocialRequest, SocialResponse},
};
use tokio::time::Instant;
use tracing::{debug, info};

use crate::{
    admission::{Side, counted_vouches, meet, owner_hops, paths},
    node::Core,
    social::request_on,
    vouches::check_carried,
};

/// Taps a node lets ask someone's node, an hour. Past it, the paths are the
/// owner's own world's and those kept from replies.
pub(crate) const MAX_ASKS_PER_HOUR: u32 = 10;
/// How long a tap waits for the other node, waiting for Tor included: a
/// first connection over Tor can take most of a minute, and the owner's
/// device waits a minute for any answer (`osp-client`'s `REPLY_TIMEOUT`),
/// so the node answers first.
const ASK_TIMEOUT: Duration = Duration::from_secs(50);
/// The least of it left to dial in, however long Tor takes to work.
const DIAL_AT_LEAST: Duration = Duration::from_secs(20);

impl Core {
    /// The paths this node knows from its owner to `actor`, nearest first.
    pub(crate) async fn how_connected(self: &Arc<Self>, actor: PublicKey) -> Result<Connection> {
        let me = self.signer.identity();
        let known = self.known()?;
        let mut paths: Vec<Path> = self
            .rooted_near_owner(&HashSet::from([actor]))?
            .iter()
            .filter(|p| p.person == actor)
            .map(|p| Path {
                hops: owner_hops(p, &known),
            })
            .collect();
        let mut names = HashMap::new();
        let side = if actor == me {
            SideFrom::NotAsked
        } else if let Some(kept) = self.db.replier_paths(&actor)? {
            paths.extend(serde_json::from_str::<Vec<Path>>(&kept)?);
            SideFrom::Replied
        } else {
            match self.ask_who_vouched_for(actor).await {
                Ok(Some(carried)) => {
                    let vouches = carried.len() as u32;
                    for c in &carried {
                        if let (Ok(voucher), Ok(Payload::Person(person))) =
                            (check_carried(c, &actor), c.profile.verify())
                        {
                            names.insert(voucher, person.name);
                        }
                    }
                    let signed: Vec<_> = carried.into_iter().map(|c| c.vouch).collect();
                    let side = Side::new(actor, &counted_vouches(&self.db, &signed)?);
                    let people: HashSet<PublicKey> = side.steps.keys().copied().collect();
                    let placings = self.rooted_near_owner(&people)?;
                    let met = meet(&side, &placings);
                    paths.extend(self::paths(&met.chains, &placings, &known));
                    SideFrom::Asked { vouches }
                }
                Ok(None) => SideFrom::NotAsked,
                Err(e) => {
                    debug!(actor = %actor.fmt_short(), "their node didn't say who vouched: {e:#}");
                    SideFrom::NoAnswer
                }
            }
        };
        let mut seen = HashSet::new();
        paths.retain(|p| seen.insert(p.clone()));
        paths.sort_by_key(Path::steps);
        let people = self.named(actor, &paths, &names)?;
        info!(
            actor = %actor.fmt_short(),
            paths = paths.len(),
            ?side,
            "how are we connected"
        );
        Ok(Connection {
            actor,
            paths,
            people,
            side,
        })
    }

    /// Everyone `paths` name, and `actor`: each with their name, from the
    /// profile held or the one their vouch came with, and, if this node
    /// holds them, where they are on the map.
    fn named(
        &self,
        actor: PublicKey,
        paths: &[Path],
        names: &HashMap<PublicKey, String>,
    ) -> Result<Vec<Named>> {
        let mut order = vec![actor];
        for hop in paths.iter().flat_map(|p| &p.hops) {
            if !order.contains(&hop.to) {
                order.push(hop.to);
            }
        }
        let mut held = HashMap::new();
        for who in &order {
            if let Some(Ok(Payload::Person(person))) = self.db.profile(who)?.map(|p| p.verify()) {
                held.insert(*who, person.name);
            }
        }
        let mut regions = self.regions_of(held.keys().copied())?;
        Ok(order
            .into_iter()
            .map(|who| Named {
                actor: who,
                name: held.get(&who).or_else(|| names.get(&who)).cloned(),
                regions: regions.remove(&who).unwrap_or_default(),
            })
            .collect())
    }

    /// Asks `actor`'s node who vouched for them, once, from a throwaway key.
    /// None when it isn't asked: no node known for them, or too many taps
    /// this hour.
    ///
    /// Over Tor, it first waits for Tor to have reached some node since it
    /// started: Tor says it's ready a few minutes before that works, at
    /// times, and "try again later" would only be tried again. A tap that
    /// dials before Tor has worked isn't counted among the hour's.
    async fn ask_who_vouched_for(&self, actor: PublicKey) -> Result<Option<Vec<CarriedVouch>>> {
        let Some(&node) = self
            .nodes_of(&actor)
            .first()
            .or(self.voucher_nodes(&actor)?.first())
        else {
            return Ok(None);
        };
        let deadline = Instant::now() + ASK_TIMEOUT;
        let working = match &self.tor {
            Some(tor) if !tor.dialled() => {
                debug!("waiting for Tor to reach a node before asking");
                tor.wait_dialled(deadline - DIAL_AT_LEAST).await
            }
            _ => true,
        };
        if working
            && !self
                .connected_asks
                .allow(self.endpoint.id(), MAX_ASKS_PER_HOUR)
        {
            debug!("too many taps asked other nodes this hour");
            return Ok(None);
        }
        let endpoint = self.throwaway_endpoint().await?;
        let asked = async {
            let conn = endpoint
                .connect(EndpointAddr::new(node), SOCIAL_ALPN)
                .await
                .context("connecting")?;
            request_on(&conn, &SocialRequest::WhoVouched).await
        };
        let answer = tokio::time::timeout_at(deadline, asked).await;
        endpoint.close().await;
        match answer.context("no answer in time")?? {
            SocialResponse::Vouches { mut vouches } => {
                vouches.truncate(MAX_VOUCHES_ANSWERED);
                Ok(Some(vouches))
            }
            SocialResponse::Error { message } => bail!("their node replied: {message}"),
            other => bail!("unexpected reply to WhoVouched: {other:?}"),
        }
    }

    /// A new key, published nowhere and never bound to the owner's: over
    /// Tor, dial-only; without Tor, relay-only, as a viewing endpoint.
    async fn throwaway_endpoint(&self) -> Result<Endpoint> {
        self.throwaways_made.fetch_add(1, Ordering::Relaxed);
        match &self.tor {
            Some(tor) => {
                tor.check_ready()?;
                tor.dial_only(SecretKey::generate(), 1).await
            }
            None => self.viewing_endpoint().await,
        }
    }
}
