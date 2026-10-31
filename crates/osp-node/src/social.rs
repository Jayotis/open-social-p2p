//! The node-to-node protocol `osp/social/1` (ADR 0003): serving our profile
//! and outbox, and asking other nodes for theirs.

use std::{
    collections::HashMap,
    fmt,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use iroh::{
    EndpointAddr, EndpointId, PublicKey,
    endpoint::{Connection, RecvStream, SendStream},
    protocol::{AcceptError, ProtocolHandler},
};
use osp_proto::{
    Payload, Person, SOCIAL_ALPN, SignedObject,
    framing::{read_frame, read_frame_max, write_frame},
    social::{
        MAX_GET_OBJECTS, MAX_OUTBOX_PAGE, MAX_REQUEST_LEN, OutboxEntry, SocialRequest,
        SocialResponse,
    },
};
use tokio::sync::Semaphore;
use tracing::{debug, info};

use crate::{
    db::now_ms,
    network,
    node::{Core, check_addr, remember},
    peers::{self, Event},
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Most requests served at once, from all peers together. Anyone who knows
/// the node's ID can connect, so this keeps a flood from using up the
/// machine. Requests over a limit get a "busy" reply.
const MAX_REQUESTS: usize = 32;
/// Most requests served at once on one connection. Nodes send theirs one
/// at a time, so this is only reached by a peer misbehaving.
const MAX_REQUESTS_PER_CONN: usize = 4;
/// Longest one request may take, reading it and replying together, so a
/// peer that stops sending or stops reading can't keep its place.
const STREAM_TIMEOUT: Duration = Duration::from_secs(60);
/// Longest we try to tell a peer we're busy.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
/// Object bytes in one outbox page, at most. Encoded for sending, that is
/// still well under the 16 MiB frame limit.
const MAX_PAGE_BYTES: usize = 8 << 20;
/// Most followers a node keeps. Each one costs a notification per post.
const MAX_FOLLOWERS: usize = 10_000;
/// Most new followers accepted per hour. Anyone can subscribe, and new
/// identities are free, so this stops a flood of them. Someone refused
/// still sees new posts, by polling; they only miss the push notification
/// until they try again (nodes do once a day).
pub(crate) const MAX_NEW_FOLLOWERS_PER_HOUR: usize = 100;
/// Followers not heard from for this long are forgotten. A live follower's
/// node subscribes again every day, and answers notifications.
pub(crate) const FOLLOWER_SILENCE: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// Followers holding a copy that one `WhoHosts` reply names, at most. A
/// small sample: it says who follows us (doc 13).
const SWARM_SAMPLE: usize = 6;
/// A follower that sent us a request within this long is named as a host
/// before those that didn't, as likely still online. A node asks the nodes
/// it follows for new posts every minute by default.
pub(crate) const HEARD_LATELY: Duration = Duration::from_secs(5 * 60);
/// Samples one follower gets per hour. More asks get only the hosts anyone
/// may know: this node, and resharers that host.
const MAX_SWARM_SAMPLES_PER_HOUR: u32 = 60;
/// Copies of one follower's that this node checks per hour (`Hosting`). A
/// follower tells us of a copy once for each post it fetches ahead of
/// time, and one already noted isn't checked again.
const MAX_HOSTING_CHECKS_PER_HOUR: u32 = 60;
const HOUR: Duration = Duration::from_secs(60 * 60);
/// How far ahead of our clock a profile or key certificate may be dated.
/// The newer one wins (ADR 0009, ADR 0010), so one dated years ahead could
/// never be replaced; this still lets clocks a few minutes apart work.
const MAX_DATED_AHEAD: Duration = Duration::from_secs(5 * 60);

/// Accepts incoming `osp/social/1` connections.
#[derive(Clone)]
pub(crate) struct SocialProtocol {
    core: Arc<Core>,
    /// Shared by every connection, for `MAX_REQUESTS`.
    requests: Arc<Semaphore>,
}

impl SocialProtocol {
    pub fn new(core: Arc<Core>) -> Self {
        Self {
            core,
            requests: Arc::new(Semaphore::new(MAX_REQUESTS)),
        }
    }
}

impl fmt::Debug for SocialProtocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SocialProtocol")
    }
}

impl ProtocolHandler for SocialProtocol {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        let remote = conn.remote_id();
        let on_this_conn = Arc::new(Semaphore::new(MAX_REQUESTS_PER_CONN));
        // Each request arrives on its own stream; serve them concurrently,
        // within the limits.
        while let Ok((send, recv)) = conn.accept_bi().await {
            let (Ok(overall), Ok(here)) = (
                self.requests.clone().try_acquire_owned(),
                on_this_conn.clone().try_acquire_owned(),
            ) else {
                // Not even reading the request keeps being busy cheap.
                debug!(remote = %remote.fmt_short(), "too many social requests; replied busy");
                tell_busy(send).await;
                continue;
            };
            let direct = network::is_direct(&conn);
            let core = self.core.clone();
            tokio::spawn(async move {
                let served =
                    tokio::time::timeout(STREAM_TIMEOUT, serve_stream(&core, remote, send, recv))
                        .await
                        .unwrap_or_else(|_| Err(anyhow::anyhow!("took too long")));
                if let Err(e) = served {
                    debug!(remote = %remote.fmt_short(), "social request failed: {e:#}");
                }
                // After it, so a node that just subscribed counts.
                core.note_heard(remote, direct);
                drop((overall, here));
            });
        }
        Ok(())
    }
}

async fn tell_busy(mut send: SendStream) {
    let resp = SocialResponse::Error {
        message: "this node is busy; try again later".to_string(),
    };
    let _ = tokio::time::timeout(BUSY_TIMEOUT, async {
        write_frame(&mut send, &resp).await?;
        send.finish()?;
        anyhow::Ok(())
    })
    .await;
}

async fn serve_stream(
    core: &Core,
    remote: EndpointId,
    mut send: SendStream,
    mut recv: RecvStream,
) -> Result<()> {
    let req: SocialRequest = read_frame_max(&mut recv, MAX_REQUEST_LEN).await?;
    let resp = match core.answer_social(remote, req).await {
        Ok(resp) => resp,
        Err(e) => SocialResponse::Error {
            message: format!("{e:#}"),
        },
    };
    write_frame(&mut send, &resp).await?;
    send.finish()?;
    Ok(())
}

impl Core {
    async fn answer_social(
        &self,
        remote: EndpointId,
        req: SocialRequest,
    ) -> Result<SocialResponse> {
        Ok(match req {
            SocialRequest::GetProfile => SocialResponse::Profile {
                profile: self.profile.clone(),
            },
            SocialRequest::GetOutbox { since, limit } => {
                let mut entries = Vec::new();
                let mut referenced: Vec<SignedObject> = Vec::new();
                let mut page_bytes = 0;
                for (seq, object) in self.db.outbox_since(since, limit.min(MAX_OUTBOX_PAGE))? {
                    // Include what each reshare points at, and its author's
                    // profile, so a reader can show it even if the author is offline.
                    let mut extra = Vec::new();
                    if let Ok(Payload::Announce(a)) = object.verify() {
                        if let Some(target) = self.db.get_object(&a.object)? {
                            extra.push(target);
                        }
                        if let Some(profile) = self.db.profile(&a.object_actor)?
                            && !referenced.contains(&profile)
                        {
                            extra.push(profile);
                        }
                    }
                    // Keep the page well inside the frame limit; the rest
                    // comes with the next page.
                    let bytes: usize = std::iter::once(&object)
                        .chain(&extra)
                        .map(|o| o.payload_bytes().len())
                        .sum();
                    if !entries.is_empty() && page_bytes + bytes > MAX_PAGE_BYTES {
                        break;
                    }
                    page_bytes += bytes;
                    entries.push(OutboxEntry { seq, object });
                    referenced.extend(extra);
                }
                SocialResponse::Outbox {
                    entries,
                    referenced,
                    latest: self.db.outbox_latest()?,
                }
            }
            SocialRequest::Subscribe {
                addr,
                shares_prefetched,
            } => {
                ensure!(addr.id == remote, "can only subscribe on your own behalf");
                check_addr(&addr)?;
                self.accept_follower(&addr, shares_prefetched)?;
                remember(&self.lookup, &addr);
                SocialResponse::Ok
            }
            SocialRequest::Notify { actor, seq } => {
                let from_followed = self
                    .db
                    .follows()?
                    .iter()
                    .any(|f| f.actor == actor && f.addr.id == remote);
                // An error tells a node we stopped following that we're not
                // listening, so it stops counting us as a live follower.
                ensure!(from_followed, "not following {}", actor.fmt_short());
                debug!(%seq, actor = %actor.fmt_short(), "notified of new posts");
                self.sync_now.notify_one();
                SocialResponse::Ok
            }
            SocialRequest::WhoHosts { blob } => {
                let mut nodes = Vec::new();
                let class = self.db.blob_class(&blob)?;
                let share = self.config.swarm.share_prefetched;
                if class.is_some_and(|c| c.served_to_anyone(share)) {
                    nodes.push(self.shareable_addr().await);
                }
                // Followers with a copy of media we host by choice, our own
                // or reshared, only to followers.
                if class.is_some_and(|c| c.is_public())
                    && self.db.is_follower(&remote)?
                    && self.swarm_samples.allow(remote, MAX_SWARM_SAMPLES_PER_HOUR)
                {
                    let mut members = self.db.swarm_members(&blob, &remote)?;
                    let [direct, relayed, not_lately] =
                        self.heard
                            .rank(&mut members, Instant::now(), self.sim.heard_lately);
                    members.truncate(SWARM_SAMPLE);
                    let named: Vec<String> = members
                        .iter()
                        .map(|a| a.id.fmt_short().to_string())
                        .collect();
                    debug!(
                        remote = %remote.fmt_short(),
                        direct, relayed, not_lately, ?named,
                        "named followers as hosts"
                    );
                    nodes.extend(members);
                }
                for addr in self.db.hosts(&blob)? {
                    let known = nodes.iter().any(|a| a.id == addr.id);
                    if !known && addr.id != remote && addr.id != self.endpoint.id() {
                        nodes.push(addr);
                    }
                }
                SocialResponse::Hosts { nodes }
            }
            SocialRequest::Hosting { blob } => {
                ensure!(
                    self.db.follower_shares(&remote)?,
                    "only a follower that shares what it prefetches can host this node's media"
                );
                ensure!(
                    self.db.blob_class(&blob)?.is_some_and(|c| c.is_public()),
                    "{} isn't media this node hosts",
                    blob.fmt_short()
                );
                // Noted before, it keeps its place and isn't checked again.
                if !self.db.in_swarm(&blob, &remote)? {
                    ensure!(
                        self.hosting_checks
                            .allow(remote, MAX_HOSTING_CHECKS_PER_HOUR),
                        "this node has checked enough of your copies this hour; try again later"
                    );
                    if let Err(e) = self.store.check_hosting(&blob, remote).await {
                        debug!(
                            node = %remote.fmt_short(),
                            blob = %blob.fmt_short(),
                            "a follower said it holds a copy, but didn't send a piece of it: {e:#}"
                        );
                        bail!(
                            "this node couldn't get a piece of {} from yours: {e:#}",
                            blob.fmt_short()
                        );
                    }
                    self.db.add_swarm_member(&blob, &remote)?;
                }
                SocialResponse::Ok
            }
            SocialRequest::Moved { profile, addr } => {
                ensure!(
                    addr.id == remote,
                    "can only announce a move on your own behalf"
                );
                check_addr(&addr)?;
                self.follow_moved(&profile, &addr)?;
                SocialResponse::Ok
            }
            SocialRequest::GetObjects { ids } => {
                ensure!(
                    ids.len() <= MAX_GET_OBJECTS,
                    "asked for {} objects; at most {MAX_GET_OBJECTS} at a time",
                    ids.len()
                );
                // From the outbox: our own posts and reshares, never what
                // we keep of others'.
                let mut objects = Vec::new();
                for id in &ids {
                    if let Some(obj) = self.db.outbox_object(id)? {
                        objects.push(obj);
                    }
                }
                SocialResponse::Objects { objects }
            }
        })
    }

    /// Points a follow at the node an identity moved to (ADR 0009). Believed
    /// only with a profile signed by that identity, listing that node, and
    /// newer than the one we have, so an old profile can't be replayed to
    /// point us back. It mustn't be dated ahead of our clock either
    /// (`profile_date`), or no later move could replace it, nor signed with a
    /// key the identity revoked (ADR 0010).
    fn follow_moved(&self, profile: &SignedObject, addr: &EndpointAddr) -> Result<()> {
        // Who it claims to be, checked by `verify_received` below.
        let actor = profile.signer();
        let follow = self
            .db
            .follows()?
            .into_iter()
            .find(|f| f.actor == actor)
            .with_context(|| format!("not following {}", actor.fmt_short()))?;
        let Payload::Person(person) = self.verify_received(profile)? else {
            bail!("not a profile");
        };
        ensure!(
            person.nodes.contains(&addr.id),
            "the profile doesn't list your node"
        );
        if follow.addr.id == addr.id {
            return Ok(()); // Told before.
        }
        let published = profile_date(&person)?;
        if let Some(stored) = self.db.profile(&actor)?
            && let Ok(Payload::Person(known)) = stored.verify()
            && let Some(known_ms) = osp_proto::parse_rfc3339_ms(&known.published)
        {
            ensure!(
                published > known_ms,
                "this profile is older than the one we have"
            );
        }
        self.db.upsert_profile(&actor, &person.name, profile)?;
        remember(&self.lookup, addr);
        // A different node: the follow starts over there (`upsert_follow`),
        // and the sync this wakes subscribes to it.
        self.db.upsert_follow(&actor, addr)?;
        self.lock_subscribed().remove(&actor);
        self.sync_now.notify_one();
        info!(
            actor = %actor.fmt_short(),
            node = %addr.id.fmt_short(),
            "a followed identity moved to another node"
        );
        Ok(())
    }

    /// Verifies an object another node sent us: its signatures, and that
    /// the key which signed it hasn't been revoked. Learns of a newer key
    /// certificate from it (ADR 0010). Call it only for identities we follow
    /// or objects we keep, so strangers can't fill the key table, and use it
    /// for everything that arrives; `SignedObject::verify` alone is for what
    /// we already stored.
    pub(crate) fn verify_received(&self, obj: &SignedObject) -> Result<Payload> {
        let (payload, key) = obj.verify_with_key()?;
        if let (Some(key), Some(cert)) = (key, obj.certificate()) {
            let cert_ms = dated(&key.published, "key certificate")?;
            if self.db.check_key(cert, &key, cert_ms)? {
                info!(
                    actor = %key.actor.fmt_short(),
                    "an identity revoked its earlier signing keys"
                );
                self.drop_revoked(&key.actor, cert_ms)?;
            }
        }
        Ok(payload)
    }

    /// Deletes what `actor`'s keys certified before `revoked_before_ms`
    /// signed, now that they're revoked (ADR 0010). If we follow `actor`, the
    /// next sync, which this wakes, asks its node for copies signed by the
    /// new key. Our own identity's objects stay: this node signs them again
    /// itself when its key changes, and its outbox needs them.
    fn drop_revoked(&self, actor: &PublicKey, revoked_before_ms: i64) -> Result<()> {
        if *actor == self.signer.identity() {
            return Ok(());
        }
        let following = self.db.follows()?.iter().any(|f| f.actor == *actor);
        let dropped = self.db.drop_revoked(actor, revoked_before_ms, following)?;
        if dropped > 0 {
            info!(
                actor = %actor.fmt_short(),
                dropped,
                "dropped what the revoked keys signed"
            );
        }
        if following {
            self.sync_now.notify_one();
        }
        Ok(())
    }

    /// Records a subscriber, and whether it shares what it prefetches. A
    /// node we already have just gets these refreshed; a new one must fit
    /// within the limits on followers.
    fn accept_follower(&self, addr: &EndpointAddr, shares: bool) -> Result<()> {
        if !self.db.is_follower(&addr.id)? {
            let now = now_ms();
            self.db
                .drop_silent_followers(now - FOLLOWER_SILENCE.as_millis() as i64)?;
            ensure!(
                self.db.follower_count()? < MAX_FOLLOWERS,
                "this node has as many followers as it can notify"
            );
            ensure!(
                self.db.new_followers_since(now - 60 * 60 * 1000)?
                    < self.sim.new_followers_per_hour,
                "this node is busy with new followers; try again later"
            );
        }
        self.db.add_follower(addr, shares)
    }

    /// Notes that `remote` sent us a request, if it's a follower, for
    /// ranking the followers named as hosts.
    fn note_heard(&self, remote: EndpointId, direct: bool) {
        match self.db.is_follower(&remote) {
            Ok(true) => self.heard.note(remote, direct, Instant::now()),
            Ok(false) => {}
            Err(e) => debug!(remote = %remote.fmt_short(), "couldn't look up a follower: {e:#}"),
        }
    }

    /// Sends one request to another node, reusing an open connection when
    /// we have one, and records how the node did (ADR 0011). An error reply
    /// still counts as an answer: the node is up and responding.
    pub(crate) async fn social_request(
        &self,
        addr: &EndpointAddr,
        req: &SocialRequest,
    ) -> Result<SocialResponse> {
        let (reached, result) = self.ask_node(addr, req).await;
        let took = result.as_ref().ok().map(|(_, took)| *took);
        peers::record(&self.db, &addr.id, Event::Request { reached, took });
        let (resp, _) = result?;
        if let SocialResponse::Error { message } = &resp {
            bail!("{} replied: {message}", addr.id.fmt_short());
        }
        Ok(resp)
    }

    /// Sends a request, and says whether the node could be reached, and how
    /// long its reply took.
    async fn ask_node(
        &self,
        addr: &EndpointAddr,
        req: &SocialRequest,
    ) -> (bool, Result<(SocialResponse, Duration)>) {
        let conn = match self.social_conn(addr).await {
            Ok(conn) => conn,
            Err(e) => return (false, Err(e)),
        };
        let started = Instant::now();
        if let Ok(resp) = request_on(&conn, req).await {
            return (true, Ok((resp, started.elapsed())));
        }
        // The cached connection may have died quietly; try once on a fresh one.
        self.forget_conn(&addr.id);
        let conn = match self.social_conn(addr).await {
            Ok(conn) => conn,
            Err(e) => return (false, Err(e)),
        };
        let started = Instant::now();
        let result = request_on(&conn, req).await;
        (true, result.map(|resp| (resp, started.elapsed())))
    }

    async fn social_conn(&self, addr: &EndpointAddr) -> Result<Connection> {
        if let Some(conn) = self.cached_conn(&addr.id) {
            return Ok(conn);
        }
        let refused = self.relays.refused_relays(addr);
        let addr = &self.relays.filter(addr);
        remember(&self.lookup, addr);
        let conn = tokio::time::timeout(
            CONNECT_TIMEOUT,
            self.endpoint.connect(addr.clone(), SOCIAL_ALPN),
        )
        .await
        .with_context(|| format!("connecting to {} timed out", addr.id.fmt_short()))
        .and_then(|r| r.with_context(|| format!("connecting to {}", addr.id.fmt_short())));
        let conn = match conn {
            Ok(conn) => conn,
            Err(e) if !refused.is_empty() => {
                return Err(e.context(format!(
                    "its address names relays this node won't use: {}{}",
                    refused.join(", "),
                    if self.config.network.relay_only {
                        " (relay-only mode uses only the relays in config.toml)"
                    } else {
                        " (only public https relays are used)"
                    }
                )));
            }
            Err(e) => return Err(e),
        };
        self.lock_conns().insert(addr.id, conn.clone());
        Ok(conn)
    }

    fn cached_conn(&self, id: &EndpointId) -> Option<Connection> {
        let mut conns = self.lock_conns();
        match conns.get(id) {
            Some(conn) if conn.close_reason().is_none() => Some(conn.clone()),
            Some(_) => {
                conns.remove(id);
                None
            }
            None => None,
        }
    }

    fn forget_conn(&self, id: &EndpointId) {
        self.lock_conns().remove(id);
    }

    fn lock_conns(
        &self,
    ) -> std::sync::MutexGuard<'_, std::collections::HashMap<EndpointId, Connection>> {
        self.conns.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// How many times each node has done something this hour, to limit it:
/// since when, and how many.
#[derive(Default)]
pub(crate) struct PerHour(Mutex<HashMap<EndpointId, (Instant, u32)>>);

impl PerHour {
    /// Counts one more for `node`, and says whether that is within `max`
    /// this hour.
    fn allow(&self, node: EndpointId, max: u32) -> bool {
        let mut counts = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        if counts.len() >= MAX_FOLLOWERS {
            counts.retain(|_, (since, _)| now.duration_since(*since) < HOUR);
        }
        let (since, count) = counts.entry(node).or_insert((now, 0));
        if now.duration_since(*since) >= HOUR {
            (*since, *count) = (now, 0);
        }
        *count += 1;
        *count <= max
    }
}

/// When each follower last sent us a request, and whether it came straight
/// from its node or through a relay, so the followers named as hosts are
/// those most likely to serve well (doc 13). Kept in memory only: after a
/// restart, followers are named at random until they're heard from again.
#[derive(Default)]
pub(crate) struct Heard(Mutex<HashMap<EndpointId, (Instant, bool)>>);

/// How a follower was last heard from, best first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Contact {
    /// Lately, straight from its node.
    Direct,
    /// Lately, through a relay: what it sends is slower, and costs the relay.
    Relayed,
    /// Not lately, or not since this node started.
    NotLately,
}

impl Heard {
    /// Notes a request from `node` at `now`, `direct` or not.
    fn note(&self, node: EndpointId, direct: bool, now: Instant) {
        let mut heard = self.lock();
        // Followers that stopped following are never noted again; this
        // forgets them.
        if heard.len() >= MAX_FOLLOWERS && !heard.contains_key(&node) {
            heard.retain(|_, (at, _)| now.duration_since(*at) < HEARD_LATELY);
        }
        if heard.len() < MAX_FOLLOWERS || heard.contains_key(&node) {
            heard.insert(node, (now, direct));
        }
    }

    /// Orders followers by how they were last heard from, within `lately`
    /// of `now`, best first, keeping the order they came in within each.
    /// Returns how many fell in each `Contact`, best first.
    fn rank(&self, followers: &mut [EndpointAddr], now: Instant, lately: Duration) -> [usize; 3] {
        let heard = self.lock();
        let contact = |addr: &EndpointAddr| match heard.get(&addr.id) {
            Some((at, direct)) if now.duration_since(*at) < lately => {
                if *direct {
                    Contact::Direct
                } else {
                    Contact::Relayed
                }
            }
            _ => Contact::NotLately,
        };
        followers.sort_by_key(contact);
        let mut counts = [0; 3];
        for addr in followers.iter() {
            counts[contact(addr) as usize] += 1;
        }
        counts
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<EndpointId, (Instant, bool)>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// A profile's date in Unix milliseconds, if it has one and it isn't
/// further ahead of our clock than `MAX_DATED_AHEAD`.
pub(crate) fn profile_date(person: &Person) -> Result<i64> {
    dated(&person.published, "profile")
}

/// A date in Unix milliseconds, if it is one and isn't further ahead of our
/// clock than `MAX_DATED_AHEAD`. `what` names the object in errors.
fn dated(published: &str, what: &str) -> Result<i64> {
    let published = osp_proto::parse_rfc3339_ms(published)
        .with_context(|| format!("the {what} has no valid date"))?;
    let ahead = Duration::from_millis(published.saturating_sub(now_ms()).max(0) as u64);
    ensure!(
        ahead <= MAX_DATED_AHEAD,
        "the {what} is dated {} ahead of our clock; is the sender's clock right?",
        humantime::format_duration(Duration::from_secs(ahead.as_secs()))
    );
    Ok(published)
}

async fn request_on(conn: &Connection, req: &SocialRequest) -> Result<SocialResponse> {
    let (mut send, mut recv) = conn.open_bi().await?;
    write_frame(&mut send, req).await?;
    send.finish()?;
    tokio::time::timeout(REQUEST_TIMEOUT, read_frame(&mut recv))
        .await
        .context("no reply in time")?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn some_addr() -> EndpointAddr {
        EndpointAddr::new(iroh::SecretKey::generate().public())
    }

    #[test]
    fn followers_heard_from_lately_come_first_directly_before_through_a_relay() {
        let heard = Heard::default();
        let [quiet, relayed, direct, earlier, unknown] = [(); 5].map(|_| some_addr());
        let start = Instant::now();
        heard.note(earlier.id, true, start);
        let later = start + HEARD_LATELY;
        heard.note(relayed.id, false, later);
        heard.note(direct.id, true, later);
        // Heard directly once, now only through a relay.
        heard.note(quiet.id, true, later);
        heard.note(quiet.id, false, later);

        let mut followers = [&unknown, &earlier, &quiet, &relayed, &direct].map(|a| a.clone());
        let counts = heard.rank(&mut followers, later + Duration::from_secs(1), HEARD_LATELY);
        assert_eq!(counts, [1, 2, 2]);
        let ids = followers.map(|a| a.id);
        // In the order they came within each.
        assert_eq!(
            ids,
            [direct.id, quiet.id, relayed.id, unknown.id, earlier.id]
        );
    }

    #[test]
    fn a_full_record_forgets_followers_not_heard_from_lately() {
        let heard = Heard::default();
        let start = Instant::now();
        let old = some_addr();
        heard.note(old.id, true, start);
        for _ in 1..MAX_FOLLOWERS {
            heard.note(some_addr().id, true, start + HEARD_LATELY);
        }
        assert_eq!(heard.lock().len(), MAX_FOLLOWERS);
        // Full: one more makes room by forgetting the old one.
        let new = some_addr();
        heard.note(new.id, true, start + HEARD_LATELY);
        assert!(heard.lock().contains_key(&new.id));
        assert!(!heard.lock().contains_key(&old.id));
        // Full of followers heard from lately: no room, so a new one isn't
        // noted, but one already noted still is.
        let noted = *heard.lock().keys().next().unwrap();
        let soon = start + HEARD_LATELY + Duration::from_secs(1);
        heard.note(some_addr().id, false, soon);
        heard.note(noted, false, soon);
        assert_eq!(heard.lock().len(), MAX_FOLLOWERS);
        assert_eq!(heard.lock()[&noted], (soon, false));
    }
}
