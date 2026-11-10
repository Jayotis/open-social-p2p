//! The node-to-node protocol `osp/social/2` (ADR 0003): serving our profile
//! and outbox, and asking other nodes for theirs.

use std::{
    collections::{HashMap, HashSet},
    fmt,
    hash::{Hash, Hasher},
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
    BlobHash, BoundEndpoint, EndpointKind, Payload, Person, SOCIAL_ALPN, SignedObject,
    framing::{read_frame, read_frame_max, write_frame},
    social::{
        HostingReshare, MAX_GET_OBJECTS, MAX_OUTBOX_PAGE, MAX_REQUEST_LEN, OutboxEntry,
        SocialRequest, SocialResponse,
    },
};
use tokio::sync::Semaphore;
use tracing::{debug, info};

use crate::{
    db::{ResharerHost, SwarmMember, now_ms},
    network,
    node::{Core, check_addr, remember},
    peers::{self, Event},
    store::Source,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// The same for a node reaching others over Tor, where a first connection
/// took up to 24 s (ADR 0014). One given up on still readies the circuit for
/// the next try.
const CONNECT_TIMEOUT_OVER_TOR: Duration = Duration::from_secs(60);
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
/// Of those, trusted followers, at most, while there are others to name
/// (ADR 0013).
const MAX_TRUSTED_NAMED: usize = 3;
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
/// Hosting resharers of one post an author's node keeps (ADR 0025). A
/// starting value: well over what it names, so a few that stop serving
/// leave others to name.
const MAX_RESHARER_HOSTS: usize = 64;
/// Hosting resharers one reply names, at most, by the day's draw.
pub(crate) const RESHARERS_NAMED: usize = 6;
/// A resharer whose copy failed this many checks in a row isn't named,
/// until one passes (ADR 0025).
pub(crate) const RESHARER_FAILS_UNNAMED: u32 = 2;
/// A host whose copy passed a check within this long may take the one place
/// for a recently verified host (ADR 0025). A starting value.
const RECENT_HOST: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const HOUR: Duration = Duration::from_secs(60 * 60);
/// How far ahead of our clock a profile or key certificate may be dated.
/// The newer one wins (ADR 0009, ADR 0010), so one dated years ahead could
/// never be replaced; this still lets clocks a few minutes apart work.
const MAX_DATED_AHEAD: Duration = Duration::from_secs(5 * 60);

/// Accepts incoming `osp/social/2` connections.
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
        // Over Tor, replying sets up our circuits to the remote first.
        #[cfg(feature = "sim")]
        if self.core.sim.tor_delay.is_some() {
            self.core.lock_tor_ready().insert(remote, Instant::now());
        }
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
            // With other endpoints, how a request came says nothing of how
            // a node's media would: that goes by the endpoint it's named by.
            let direct = match self.core.endpoints {
                Some(_) => Some(false),
                None => network::is_direct(&conn),
            };
            let core = self.core.clone();
            tokio::spawn(async move {
                let served =
                    tokio::time::timeout(STREAM_TIMEOUT, serve_stream(&core, remote, send, recv))
                        .await
                        .unwrap_or_else(|_| Err(anyhow::anyhow!("took too long")));
                if let Err(e) = served {
                    debug!(remote = %remote.fmt_short(), "social request failed: {e:#}");
                }
                core.heard.note(remote, direct, Instant::now());
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
            SocialRequest::Endpoints { direct } => self.answer_endpoints(remote, direct).await?,
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
                    match object.verify() {
                        Ok(Payload::Announce(a)) => {
                            if let Some(target) = self.db.get_object(&a.object)? {
                                extra.push(target);
                            }
                            if let Some(profile) = self.db.profile(&a.object_actor)?
                                && !referenced.contains(&profile)
                            {
                                extra.push(profile);
                            }
                        }
                        // A forward carries the author's delete (ADR 0019).
                        Ok(Payload::ForwardDelete(f)) => {
                            extra.extend(self.db.delete_by_id(&f.delete)?);
                        }
                        _ => {}
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
                media,
                direct,
            } => {
                ensure!(addr.id == remote, "can only subscribe on your own behalf");
                check_addr(&addr)?;
                self.accept_follower(
                    &addr,
                    shares_prefetched,
                    media.as_deref(),
                    direct.as_deref(),
                )?;
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
                self.sync_soon(&actor);
                SocialResponse::Ok
            }
            SocialRequest::WhoHosts { blob } => {
                // A node with other endpoints names every host by the endpoint
                // that serves its media; it has no other address to give.
                let bound = self.endpoints.is_some();
                let mut nodes = Vec::new();
                let mut hosts = Vec::new();
                let class = self.db.blob_class(&blob)?;
                let share = self.config.swarm.share_prefetched;
                if class.is_some_and(|c| c.served_to_anyone(share)) {
                    match bound {
                        true => hosts.extend(self.own_host_for(&remote).await?),
                        false => nodes.push(self.shareable_addr().await),
                    }
                }
                let public = class.is_some_and(|c| c.is_public());
                // Followers with a copy of media we host by choice, our own
                // or reshared, only to followers.
                let mut members = Vec::new();
                let mut sample = Vec::new();
                let sampled = public
                    && self.db.is_follower(&remote)?
                    && self.swarm_samples.allow(remote, MAX_SWARM_SAMPLES_PER_HOUR);
                if sampled {
                    members = self.db.swarm_members(&blob, &remote)?;
                    members.retain(|m| !bound || m.host.is_some());
                    let picked = self.heard.pick_hosts(
                        members.clone(),
                        Instant::now(),
                        self.sim.heard_lately,
                    );
                    let [direct, relayed, not_lately] = picked.pool;
                    let named: Vec<String> = picked
                        .named
                        .iter()
                        .map(|m| m.addr.id.fmt_short().to_string())
                        .collect();
                    debug!(
                        remote = %remote.fmt_short(),
                        direct, relayed, not_lately, trusted = picked.trusted, ?named,
                        "named followers as hosts"
                    );
                    sample = picked.named;
                }
                // Hosting resharers that may be listed, to anyone (ADR 0025),
                // by their media endpoints. Only a node with other endpoints
                // names them: they serve nothing on their public IDs.
                let mut resharers = Vec::new();
                if bound && public {
                    resharers = self.resharers_to_name(&blob)?;
                    resharers.retain(|h| h.binding.public != remote);
                }
                // At most one place for a recently verified, under-used host:
                // in the followers' sample for a follower, else among the
                // resharers. None kept when none qualifies.
                if public && self.config.swarm.recent_host_place {
                    let named: HashSet<EndpointId> = sample
                        .iter()
                        .map(|m| m.addr.id)
                        .chain(resharers.iter().map(|h| h.binding.public))
                        .collect();
                    match self.recent_host(&blob, remote, bound, &members, &named)? {
                        Some(Recent::Follower(member)) => {
                            give_place(&mut sample, SWARM_SAMPLE, member, |m| m.trusted)
                        }
                        Some(Recent::Resharer(host)) if sampled => {
                            // A follower's sample holds followers' copies;
                            // a resharer's place is taken from it.
                            if sample.len() >= SWARM_SAMPLE
                                && let Some(i) = sample.iter().rposition(|m| !m.trusted)
                            {
                                sample.remove(i);
                            }
                            resharers.insert(0, host);
                        }
                        Some(Recent::Resharer(host)) => {
                            give_place(&mut resharers, RESHARERS_NAMED, host, |_| false)
                        }
                        None => {}
                    }
                }
                let mut named_now: Vec<EndpointId> = Vec::new();
                for member in sample {
                    named_now.push(member.addr.id);
                    match member.host.filter(|_| bound) {
                        Some(host) => hosts.push(host),
                        None => nodes.push(member.addr),
                    }
                }
                for host in resharers {
                    if !hosts
                        .iter()
                        .any(|h| h.binding.public == host.binding.public)
                    {
                        named_now.push(host.binding.public);
                        hosts.push(host);
                    }
                }
                self.named_hosts.note(&named_now);
                for addr in self.db.hosts(&blob)? {
                    if addr.id == remote || addr.id == self.endpoint.id() {
                        continue;
                    }
                    if !bound {
                        if !nodes.iter().any(|a| a.id == addr.id) {
                            nodes.push(addr);
                        }
                    } else if !hosts.iter().any(|h| h.binding.public == addr.id)
                        && let Some(media) = self.db.endpoint_of(&addr.id, EndpointKind::Media)?
                    {
                        hosts.push(media);
                    }
                }
                match bound {
                    true => SocialResponse::BoundHosts { hosts },
                    false => SocialResponse::Hosts { nodes },
                }
            }
            SocialRequest::Hosting {
                blob,
                reshare: Some(reshare),
            } => {
                self.resharer_hosting(remote, &blob, &reshare).await?;
                SocialResponse::Ok
            }
            SocialRequest::Hosting {
                blob,
                reshare: None,
            } => {
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
                    let source = self.hosting_source(&remote)?;
                    if let Err(e) = self.store.check_hosting(&blob, source).await {
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
                // we keep of others'. One we deleted is answered with its
                // `Delete` (ADR 0019).
                let mut objects = Vec::new();
                let me = self.signer.identity();
                for id in &ids {
                    if let Some(obj) = self.db.outbox_object(id)? {
                        objects.push(obj);
                    } else if let Some(delete) = self.db.delete_of(id, &me)? {
                        objects.push(delete);
                    }
                }
                SocialResponse::Objects { objects }
            }
            SocialRequest::AskTrust { profile, note } => {
                self.asked_trust(remote, &profile, &note)?;
                SocialResponse::Ok
            }
            SocialRequest::AskListing { profile } => {
                self.asked_listing(remote, &profile)?;
                SocialResponse::Ok
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
        self.db.upsert_profile(&person, profile)?;
        remember(&self.lookup, addr);
        // A different node: the follow starts over there (`upsert_follow`),
        // and the sync this wakes subscribes to it.
        self.db.upsert_follow(&actor, addr)?;
        self.lock_subscribed().remove(&actor);
        self.sync_soon(&actor);
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

    /// A resharer tells this node it hosts a copy of one of its posts (ADR
    /// 0025). Its `Announce` and its owner's profile are checked; then, if
    /// it's new, a piece of `blob` from the media endpoint the profile gives.
    /// One noted before isn't checked again here, the daily rechecks do
    /// that, but gets its newest profile's endpoint and listing choice.
    async fn resharer_hosting(
        &self,
        remote: EndpointId,
        blob: &BlobHash,
        reshare: &HostingReshare,
    ) -> Result<()> {
        let me = self.signer.identity();
        let Payload::Announce(a) = reshare.announce.verify()? else {
            bail!("that isn't a reshare");
        };
        ensure!(a.hosting, "that reshare doesn't host a copy");
        ensure!(
            a.object_actor == me,
            "that reshare isn't of this node's posts"
        );
        ensure!(
            reshare.profile.signer() == a.actor,
            "the profile isn't the resharer's"
        );
        let Payload::Person(person) = reshare.profile.verify()? else {
            bail!("that isn't a profile");
        };
        profile_date(&person)?;
        ensure!(
            person.nodes.contains(&remote),
            "the profile doesn't list your node"
        );
        let note = match self.db.outbox_object(&a.object)? {
            Some(obj) => match obj.verify()? {
                Payload::Note(note) => note,
                _ => bail!("{} isn't one of this node's posts", a.object.fmt_short()),
            },
            None => bail!("{} isn't one of this node's posts", a.object.fmt_short()),
        };
        let size = note
            .sized_blobs()
            .find_map(|(b, size)| (b == *blob).then_some(size))
            .with_context(|| format!("{} isn't media of that post", blob.fmt_short()))?;
        let media = person
            .media_endpoint(&remote)
            .context("the profile gives no media endpoint for your node")?
            .clone();
        let blobs: Vec<BlobHash> = note.blobs().collect();
        let host = ResharerHost {
            node: remote,
            post: a.object,
            actor: a.actor,
            announce: reshare.announce.id(),
            media,
            discoverable: person.discoverable,
            fails: 0,
            checked_ms: now_ms(),
        };
        if self.db.is_resharer_host(&a.object, &remote)? {
            return self.db.note_resharer_host(&host, &blobs);
        }
        ensure!(
            self.db.resharer_hosts_of(&a.object)? < MAX_RESHARER_HOSTS,
            "this node keeps as many hosting resharers of that post as it notes"
        );
        ensure!(
            self.hosting_checks
                .allow(remote, MAX_HOSTING_CHECKS_PER_HOUR),
            "this node has checked enough of your copies this hour; try again later"
        );
        let source = Source {
            node: remote,
            endpoint: host.media.endpoint,
            from_direct: false,
        };
        if let Err(e) = self.store.check_piece(blob, size, source).await {
            debug!(
                resharer = %a.actor.fmt_short(),
                "a resharer said it hosts a copy, but didn't send a piece of it: {e:#}"
            );
            bail!(
                "this node couldn't get a piece of {} from your media endpoint: {e:#}",
                blob.fmt_short()
            );
        }
        self.db.note_resharer_host(&host, &blobs)?;
        info!(
            resharer = %a.actor.fmt_short(),
            post = %a.object.fmt_short(),
            listed = person.discoverable,
            "a resharer hosts a copy of a post"
        );
        Ok(())
    }

    /// The host for the one place in a `WhoHosts` reply for a recently
    /// verified, under-used host (ADR 0025): of those `named` doesn't hold
    /// already, the one this node has named least often. Followers in
    /// `members` qualify if their copy was checked within `RECENT_HOST` and
    /// they were heard from lately; hosting resharers that may be listed if
    /// their last check passed within it. None if nobody qualifies.
    fn recent_host(
        &self,
        blob: &BlobHash,
        remote: EndpointId,
        bound: bool,
        members: &[SwarmMember],
        named: &HashSet<EndpointId>,
    ) -> Result<Option<Recent>> {
        let since = now_ms() - RECENT_HOST.as_millis() as i64;
        let now = Instant::now();
        let mut candidates: Vec<(EndpointId, Recent)> = members
            .iter()
            .filter(|m| {
                m.verified_ms >= since
                    && self.heard.contact(&m.addr.id, now, self.sim.heard_lately)
                        != Contact::NotLately
            })
            .map(|m| (m.addr.id, Recent::Follower(m.clone())))
            .collect();
        if bound {
            for host in self.db.named_resharers(blob, 1)? {
                if host.checked_ms >= since && host.node != remote {
                    let bound = BoundEndpoint {
                        addr: EndpointAddr::new(host.media.endpoint),
                        binding: host.media,
                    };
                    candidates.push((host.node, Recent::Resharer(bound)));
                }
            }
        }
        candidates.retain(|(id, _)| !named.contains(id));
        let picked = self.named_hosts.least(candidates);
        if let Some((id, _)) = &picked {
            debug!(host = %id.fmt_short(), "gave the place for a recently verified host");
        }
        Ok(picked.map(|(_, recent)| recent))
    }

    /// The hosting resharers of `blob` this node names to anyone (ADR
    /// 0025), by their media endpoints: those that may be listed and whose
    /// last 2 checks didn't both fail. At most `RESHARERS_NAMED`, by a draw
    /// that holds for the day and changes the next.
    pub(crate) fn resharers_to_name(&self, blob: &BlobHash) -> Result<Vec<BoundEndpoint>> {
        let mut named = self.db.named_resharers(blob, RESHARER_FAILS_UNNAMED)?;
        let day = now_ms() / (24 * 60 * 60 * 1000);
        named.sort_by_key(|host| {
            let mut h = std::hash::DefaultHasher::new();
            (blob.as_bytes(), day, host.node.as_bytes()).hash(&mut h);
            h.finish()
        });
        Ok(named
            .into_iter()
            .take(RESHARERS_NAMED)
            .map(|host| BoundEndpoint {
                addr: EndpointAddr::new(host.media.endpoint),
                binding: host.media,
            })
            .collect())
    }

    /// Where to get a piece of a blob from a follower that says it holds a
    /// copy: the endpoint it subscribed with, if it has other endpoints.
    fn hosting_source(&self, follower: &EndpointId) -> Result<Source> {
        let Some(host) = self.db.follower_host(follower)? else {
            ensure!(
                self.endpoints.is_none(),
                "this node doesn't know an endpoint of yours to check it through"
            );
            return Ok((*follower).into());
        };
        self.source_named(&host)
            .context("the endpoint you subscribed with doesn't check out")
    }

    /// Records a subscriber, whether it shares what it prefetches, and its
    /// endpoints. A node we already have just gets these refreshed; a new one
    /// must fit within the limits on followers. It's named to other nodes by
    /// `direct`, which it means them to know, or else `media`.
    fn accept_follower(
        &self,
        addr: &EndpointAddr,
        shares: bool,
        media: Option<&BoundEndpoint>,
        direct: Option<&BoundEndpoint>,
    ) -> Result<()> {
        if let Some(bound) = media {
            self.check_endpoint(&addr.id, EndpointKind::Media, bound)?;
        }
        if let Some(bound) = direct {
            self.check_endpoint(&addr.id, EndpointKind::Direct, bound)?;
        }
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
        self.db.add_follower(addr, shares)?;
        if let Some(bound) = media {
            self.keep_endpoint(&addr.id, EndpointKind::Media, bound)?;
        }
        if let Some(bound) = direct {
            self.keep_endpoint(&addr.id, EndpointKind::Direct, bound)?;
        }
        self.db.set_follower_host(&addr.id, direct.or(media))
    }

    /// Sends one request to another node, reusing an open connection when
    /// we have one, and records how the node did (ADR 0011). An error reply
    /// still counts as an answer: the node is up and responding. While our
    /// own Tor isn't ready, messages wait; that says nothing of the node.
    pub(crate) async fn social_request(
        &self,
        addr: &EndpointAddr,
        req: &SocialRequest,
    ) -> Result<SocialResponse> {
        if let Some(tor) = &self.tor {
            tor.check_ready()?;
        }
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
            #[cfg(feature = "sim")]
            self.tor_request_delay(&addr.id, &resp).await;
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
        #[cfg(feature = "sim")]
        if let Ok(resp) = &result {
            self.tor_request_delay(&addr.id, resp).await;
        }
        (true, result.map(|resp| (resp, started.elapsed())))
    }

    async fn social_conn(&self, addr: &EndpointAddr) -> Result<Connection> {
        if let Some(conn) = self.cached_conn(&addr.id) {
            return Ok(conn);
        }
        // Over Tor a node's ID is all it takes, and nothing else in an address
        // from someone else is used.
        let addr = &match self.tor {
            Some(_) => EndpointAddr::new(addr.id),
            None => addr.clone(),
        };
        let refused = self.relays.refused_relays(addr);
        let addr = &self.relays.filter(addr);
        remember(&self.lookup, addr);
        let connect = async {
            #[cfg(feature = "sim")]
            self.tor_connect_delay(&addr.id).await;
            self.endpoint.connect(addr.clone(), SOCIAL_ALPN).await
        };
        let timeout = match self.messages_over_tor() {
            true => CONNECT_TIMEOUT_OVER_TOR,
            false => CONNECT_TIMEOUT,
        };
        let conn = tokio::time::timeout(timeout, connect)
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

    /// Whether messages to other nodes take seconds, and a first connection
    /// may take up to a minute (ADR 0014).
    pub(crate) fn messages_over_tor(&self) -> bool {
        #[cfg(feature = "sim")]
        if self.sim.tor_delay.is_some() {
            return true;
        }
        self.tor.is_some()
    }

    /// Whether a request to `node` is a round trip or two away, not a first
    /// connection over Tor: someone waiting waits only for those.
    pub(crate) fn answers_quickly(&self, node: &EndpointId) -> bool {
        if !self.messages_over_tor() || self.cached_conn(node).is_some() {
            return true;
        }
        #[cfg(feature = "sim")]
        if self.sim.tor_delay.is_some() {
            return self
                .lock_tor_ready()
                .get(node)
                .is_some_and(|ready| *ready <= Instant::now());
        }
        self.tor.as_ref().is_some_and(|tor| tor.has_stream(node))
    }

    /// Sets up a connection to `node` in the background, so someone waiting
    /// next time doesn't wait for it.
    pub(crate) fn ready_conn(&self, node: EndpointId) {
        #[cfg(feature = "sim")]
        if let Some(tor) = &self.sim.tor_delay {
            let ready = Instant::now() + tor.first_connect();
            self.lock_tor_ready().entry(node).or_insert(ready);
            return;
        }
        if !lock(&self.readying).insert(node) {
            return;
        }
        let endpoint = self.endpoint.clone();
        let (conns, readying) = (self.conns.clone(), self.readying.clone());
        tokio::spawn(async move {
            let connect = endpoint.connect(EndpointAddr::new(node), SOCIAL_ALPN);
            let connected = tokio::time::timeout(CONNECT_TIMEOUT_OVER_TOR, connect).await;
            if let Ok(Ok(conn)) = connected {
                lock(&conns).insert(node, conn);
            }
            lock(&readying).remove(&node);
        });
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

    pub(crate) fn forget_conn(&self, id: &EndpointId) {
        self.lock_conns().remove(id);
    }

    fn lock_conns(&self) -> std::sync::MutexGuard<'_, HashMap<EndpointId, Connection>> {
        lock(&self.conns)
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// Connections being set up in the background (`Core::ready_conn`).
pub(crate) type Readying = Arc<Mutex<HashSet<EndpointId>>>;

/// The harness's Tor-like delays (`SimSettings::tor_delay`).
#[cfg(feature = "sim")]
impl Core {
    /// Waits as connecting to `node` over Tor would: for the circuits to it
    /// if they aren't ready, then one round trip. Circuits are set up in
    /// the background, so a connection given up on still readies them.
    async fn tor_connect_delay(&self, node: &EndpointId) {
        let Some(tor) = &self.sim.tor_delay else {
            return;
        };
        let now = Instant::now();
        let ready = *self
            .lock_tor_ready()
            .entry(*node)
            .or_insert_with(|| now + tor.first_connect());
        tokio::time::sleep(ready.saturating_duration_since(now)).await;
        tokio::time::sleep(tor.round_trip(&self.endpoint.id(), node)).await;
    }

    /// Waits as a request to `node` with reply `resp` would take over Tor,
    /// beyond what it took.
    async fn tor_request_delay(&self, node: &EndpointId, resp: &SocialResponse) {
        if let Some(tor) = &self.sim.tor_delay {
            let bytes = serde_json::to_vec(resp).map_or(0, |v| v.len());
            tokio::time::sleep(tor.request(&self.endpoint.id(), node, bytes)).await;
        }
    }

    pub(crate) fn lock_tor_ready(&self) -> std::sync::MutexGuard<'_, HashMap<EndpointId, Instant>> {
        self.tor_ready.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// How many times each node has done something this hour, to limit it:
/// since when, and how many.
#[derive(Default)]
pub(crate) struct PerHour(Mutex<HashMap<EndpointId, (Instant, u32)>>);

impl PerHour {
    /// Counts one more for `node`, and says whether that is within `max`
    /// this hour.
    pub(crate) fn allow(&self, node: EndpointId, max: u32) -> bool {
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
/// Other nodes are noted too, as checking which are followers would read
/// the database on every request; only followers are ranked.
#[derive(Default)]
pub(crate) struct Heard(Mutex<HashMap<EndpointId, (Instant, bool)>>);

/// How a follower was last heard from, best first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Contact {
    /// Lately, straight from its node.
    Direct,
    /// Lately, through a relay: what it sends is slower, and costs the relay.
    Relayed,
    /// Not lately, or not since this node started.
    NotLately,
}

/// The followers with a copy that one `WhoHosts` reply names.
pub(crate) struct Picked {
    pub named: Vec<SwarmMember>,
    /// How many of the followers they were picked from fell in each
    /// `Contact`, best first.
    pub pool: [usize; 3],
    /// How many of those named are trusted.
    pub trusted: usize,
}

impl Heard {
    /// Notes a request from `node` at `now`, `direct` or not. Without a
    /// selected path, it came the way `node` was last heard, or directly if
    /// it's new: nothing says it came through a relay.
    fn note(&self, node: EndpointId, direct: Option<bool>, now: Instant) {
        let mut heard = self.lock();
        // Nodes that stopped asking are never noted again; this forgets
        // them. A full record of nodes heard from lately notes no new ones:
        // they're ranked as after a restart.
        if heard.len() >= MAX_FOLLOWERS && !heard.contains_key(&node) {
            heard.retain(|_, (at, _)| now.duration_since(*at) < HEARD_LATELY);
        }
        if heard.len() < MAX_FOLLOWERS || heard.contains_key(&node) {
            let last = heard.get(&node).map(|&(_, direct)| direct);
            heard.insert(node, (now, direct.or(last).unwrap_or(true)));
        }
    }

    /// How `node` was last heard from, within `lately` of `now`.
    pub(crate) fn contact(&self, node: &EndpointId, now: Instant, lately: Duration) -> Contact {
        contact(&self.lock(), node, now, lately)
    }

    /// Picks the followers with a copy that one `WhoHosts` reply names, at
    /// most `SWARM_SAMPLE`, from `members` in random order (trusted ones:
    /// ADR 0013). First up to `MAX_TRUSTED_NAMED`
    /// trusted followers heard from within `lately` of `now`, at random;
    /// then the others as for anyone, those heard from lately first, but
    /// no more trusted ones than that unless nobody else is left to fill
    /// the places. Those picked are then put in
    /// order of how they were heard from, directly before through a relay,
    /// keeping the order they came in within each, so trusted followers
    /// aren't always among the first a downloader asks. A follower named by
    /// an endpoint is ranked by that endpoint, not by how its last message
    /// came: over Tor, that says nothing of how its media would.
    pub(crate) fn pick_hosts(
        &self,
        members: Vec<SwarmMember>,
        now: Instant,
        lately: Duration,
    ) -> Picked {
        let contacts: Vec<Contact> = {
            let heard = self.lock();
            members
                .iter()
                .map(|member| {
                    let contact = contact(&heard, &member.addr.id, now, lately);
                    match &member.host {
                        _ if contact == Contact::NotLately => contact,
                        Some(host) if host.binding.kind == EndpointKind::Direct => Contact::Direct,
                        Some(_) => Contact::Relayed,
                        None => contact,
                    }
                })
                .collect()
        };
        let mut picked = vec![false; members.len()];
        let mut trusted = 0;
        for (i, member) in members.iter().enumerate() {
            if trusted < MAX_TRUSTED_NAMED && member.trusted && contacts[i] != Contact::NotLately {
                picked[i] = true;
                trusted += 1;
            }
        }
        let mut others: Vec<usize> = (0..members.len()).filter(|&i| !picked[i]).collect();
        others.sort_by_key(|&i| contacts[i]);
        let mut count = trusted;
        let mut held_back = Vec::new();
        for i in others {
            if count == SWARM_SAMPLE {
                break;
            }
            if members[i].trusted {
                if trusted == MAX_TRUSTED_NAMED {
                    held_back.push(i);
                    continue;
                }
                trusted += 1;
            }
            picked[i] = true;
            count += 1;
        }
        // Places nobody else holding a copy can fill.
        for i in held_back.into_iter().take(SWARM_SAMPLE - count) {
            picked[i] = true;
            trusted += 1;
        }
        let mut pool = [0; 3];
        for c in &contacts {
            pool[*c as usize] += 1;
        }
        let mut named: Vec<(Contact, SwarmMember)> = members
            .into_iter()
            .zip(contacts)
            .zip(picked)
            .filter_map(|((member, contact), picked)| picked.then_some((contact, member)))
            .collect();
        named.sort_by_key(|(contact, _)| *contact);
        Picked {
            named: named.into_iter().map(|(_, member)| member).collect(),
            pool,
            trusted,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<EndpointId, (Instant, bool)>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The host for a reply's place for a recently verified host.
enum Recent {
    Follower(SwarmMember),
    Resharer(BoundEndpoint),
}

/// Puts `host` among `named`, at most `most` of them: in place of the last
/// one `fixed` doesn't hold in place if they're full. Nothing changes when
/// every place is held (trusted followers', ADR 0013).
fn give_place<T>(named: &mut Vec<T>, most: usize, host: T, fixed: impl Fn(&T) -> bool) {
    if named.len() >= most {
        match named.iter().rposition(|h| !fixed(h)) {
            Some(i) => {
                named.remove(i);
            }
            None => return,
        }
    }
    named.push(host);
}

/// How often this node has named each host in a `WhoHosts` reply, since it
/// started, so the place for a recently verified host goes to one named
/// least. In memory only.
#[derive(Default)]
pub(crate) struct NamedHosts(Mutex<HashMap<EndpointId, u32>>);

impl NamedHosts {
    fn note(&self, named: &[EndpointId]) {
        let mut counts = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if counts.len() >= MAX_FOLLOWERS {
            counts.clear();
        }
        for id in named {
            *counts.entry(*id).or_default() += 1;
        }
    }

    /// The candidate named least often, the first of those named as few times.
    fn least<T>(&self, candidates: Vec<(EndpointId, T)>) -> Option<(EndpointId, T)> {
        let counts = self.0.lock().unwrap_or_else(|e| e.into_inner());
        candidates
            .into_iter()
            .enumerate()
            .min_by_key(|(i, (id, _))| (counts.get(id).copied().unwrap_or(0), *i))
            .map(|(_, c)| c)
    }
}

/// How `node` was last heard from, by `heard`, within `lately` of `now`.
fn contact(
    heard: &HashMap<EndpointId, (Instant, bool)>,
    node: &EndpointId,
    now: Instant,
    lately: Duration,
) -> Contact {
    match heard.get(node) {
        Some((at, direct)) if now.duration_since(*at) < lately => {
            if *direct {
                Contact::Direct
            } else {
                Contact::Relayed
            }
        }
        _ => Contact::NotLately,
    }
}

/// A profile's date in Unix milliseconds, if it has one and it isn't
/// further ahead of our clock than `MAX_DATED_AHEAD`.
pub(crate) fn profile_date(person: &Person) -> Result<i64> {
    dated(&person.published, "profile")
}

/// A date in Unix milliseconds, if it is one and isn't further ahead of our
/// clock than `MAX_DATED_AHEAD`. `what` names the object in errors.
pub(crate) fn dated(published: &str, what: &str) -> Result<i64> {
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

pub(crate) async fn request_on(conn: &Connection, req: &SocialRequest) -> Result<SocialResponse> {
    let (mut send, mut recv) = conn.open_bi().await?;
    write_frame(&mut send, req).await?;
    send.finish()?;
    tokio::time::timeout(REQUEST_TIMEOUT, read_frame(&mut recv))
        .await
        .context("no reply in time")?
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    fn some_addr() -> EndpointAddr {
        EndpointAddr::new(iroh::SecretKey::generate().public())
    }

    fn member(addr: &EndpointAddr, trusted: bool) -> SwarmMember {
        SwarmMember {
            addr: addr.clone(),
            trusted,
            host: None,
            verified_ms: 0,
        }
    }

    fn untrusted<const N: usize>(followers: [&EndpointAddr; N]) -> Vec<SwarmMember> {
        followers.iter().map(|a| member(a, false)).collect()
    }

    fn ids(picked: &Picked) -> Vec<EndpointId> {
        picked.named.iter().map(|m| m.addr.id).collect()
    }

    /// `n` followers, trusted or not, noted as heard from at `at`, directly.
    fn heard_from(heard: &Heard, n: usize, trusted: bool, at: Instant) -> Vec<SwarmMember> {
        (0..n)
            .map(|_| {
                let addr = some_addr();
                heard.note(addr.id, Some(true), at);
                member(&addr, trusted)
            })
            .collect()
    }

    /// The followers in `members` that are trusted.
    fn trusted_in(members: &[SwarmMember]) -> HashSet<EndpointId> {
        members
            .iter()
            .filter(|m| m.trusted)
            .map(|m| m.addr.id)
            .collect()
    }

    #[test]
    fn at_most_three_trusted_followers_are_named_from_those_heard_from_lately() {
        let heard = Heard::default();
        let now = Instant::now();
        let mut members = heard_from(&heard, 10, true, now);
        members.extend(heard_from(&heard, 10, false, now));
        let trusted = trusted_in(&members);
        for _ in 0..20 {
            members.reverse();
            let picked = heard.pick_hosts(members.clone(), now, HEARD_LATELY);
            assert_eq!(picked.named.len(), SWARM_SAMPLE);
            assert_eq!(picked.trusted, MAX_TRUSTED_NAMED);
            let named = ids(&picked).into_iter().filter(|id| trusted.contains(id));
            assert_eq!(named.count(), MAX_TRUSTED_NAMED);
        }

        // With too few others, more trusted followers fill the places left.
        let mut members = heard_from(&heard, 7, true, now);
        members.extend(heard_from(&heard, 1, false, now));
        let picked = heard.pick_hosts(members, now, HEARD_LATELY);
        assert_eq!((picked.named.len(), picked.trusted), (SWARM_SAMPLE, 5));
    }

    #[test]
    fn trusted_followers_arent_put_ahead_of_others_heard_the_same_way() {
        let heard = Heard::default();
        let now = Instant::now();
        let [a, x, relayed, y, b, z] = [(); 6].map(|_| some_addr());
        for addr in [&a, &x, &y, &b, &z] {
            heard.note(addr.id, Some(true), now);
        }
        heard.note(relayed.id, Some(false), now);
        let members = [(&a, false), (&x, true), (&relayed, false)]
            .into_iter()
            .chain([(&y, true), (&b, false), (&z, true)])
            .map(|(addr, trusted)| member(addr, trusted))
            .collect();
        // In the order they came, as random from the database, within each.
        let picked = heard.pick_hosts(members, now, HEARD_LATELY);
        assert_eq!(picked.trusted, 3);
        assert_eq!(ids(&picked), [a.id, x.id, y.id, b.id, z.id, relayed.id]);
    }

    #[test]
    fn trusted_followers_not_heard_from_lately_arent_preferred() {
        let heard = Heard::default();
        let now = Instant::now();
        let long_ago = now - 2 * HEARD_LATELY;
        let mut members = heard_from(&heard, 3, true, long_ago);
        members.extend(heard_from(&heard, 6, false, now));
        let picked = heard.pick_hosts(members.clone(), now, HEARD_LATELY);
        assert_eq!(picked.trusted, 0);
        assert!(
            ids(&picked)
                .iter()
                .all(|id| !trusted_in(&members).contains(id))
        );

        // Nor held back: with none heard from lately, as after a restart,
        // they're named like anyone else.
        // Here they came first, so they're named first.
        let picked = heard.pick_hosts(members, now + 2 * HEARD_LATELY, HEARD_LATELY);
        assert_eq!(picked.pool, [0, 0, 9]);
        assert_eq!((picked.named.len(), picked.trusted), (SWARM_SAMPLE, 3));
    }

    #[test]
    fn followers_heard_from_lately_come_first_directly_before_through_a_relay() {
        let heard = Heard::default();
        let [quiet, relayed, direct, earlier, unknown] = [(); 5].map(|_| some_addr());
        let start = Instant::now();
        heard.note(earlier.id, Some(true), start);
        let later = start + HEARD_LATELY;
        heard.note(relayed.id, Some(false), later);
        heard.note(direct.id, Some(true), later);
        // Heard directly once, now only through a relay.
        heard.note(quiet.id, Some(true), later);
        heard.note(quiet.id, Some(false), later);

        let followers = untrusted([&unknown, &earlier, &quiet, &relayed, &direct]);
        let picked = heard.pick_hosts(followers, later + Duration::from_secs(1), HEARD_LATELY);
        assert_eq!(picked.pool, [1, 2, 2]);
        // In the order they came within each.
        assert_eq!(
            ids(&picked),
            [direct.id, quiet.id, relayed.id, unknown.id, earlier.id]
        );
    }

    /// Over Tor every request comes the same way. What tells followers apart
    /// is the endpoint each is named by: its direct one is worth trying first.
    #[test]
    fn followers_named_by_an_endpoint_are_ranked_by_it() {
        let public = iroh::SecretKey::generate();
        let bound = |kind| BoundEndpoint {
            binding: osp_proto::EndpointBinding::new(
                &public,
                &iroh::SecretKey::generate(),
                kind,
                1,
            ),
            addr: some_addr(),
        };
        let heard = Heard::default();
        let now = Instant::now();
        let [relayed, direct, quiet] = [(); 3].map(|_| some_addr());
        for addr in [&relayed, &direct] {
            heard.note(addr.id, Some(false), now);
        }
        let with_host = |addr, kind| SwarmMember {
            host: Some(bound(kind)),
            ..member(addr, false)
        };
        let members = vec![
            with_host(&quiet, EndpointKind::Direct),
            with_host(&relayed, EndpointKind::Media),
            with_host(&direct, EndpointKind::Direct),
        ];
        let picked = heard.pick_hosts(members, now, HEARD_LATELY);
        assert_eq!(picked.pool, [1, 1, 1]);
        assert_eq!(ids(&picked), [direct.id, relayed.id, quiet.id]);
    }

    /// The place for a recently verified host (ADR 0025) takes the last
    /// untrusted follower's place in a full sample, never a trusted one's,
    /// and adds nothing when there's no host for it.
    #[test]
    fn the_recent_host_place_never_takes_a_trusted_followers_place() {
        let addrs: Vec<EndpointAddr> = (0..SWARM_SAMPLE).map(|_| some_addr()).collect();
        let newcomer = member(&some_addr(), false);

        // Room left: it's added.
        let mut sample: Vec<SwarmMember> = addrs[..2].iter().map(|a| member(a, false)).collect();
        give_place(&mut sample, SWARM_SAMPLE, newcomer.clone(), |m| m.trusted);
        assert_eq!(
            ids(&Picked {
                named: sample,
                pool: [0; 3],
                trusted: 0
            })
            .len(),
            3
        );

        // Full: the last untrusted one gives way.
        let mut sample: Vec<SwarmMember> = addrs
            .iter()
            .enumerate()
            .map(|(i, a)| member(a, i < 3 || i == SWARM_SAMPLE - 1))
            .collect();
        give_place(&mut sample, SWARM_SAMPLE, newcomer.clone(), |m| m.trusted);
        let named: Vec<EndpointId> = sample.iter().map(|m| m.addr.id).collect();
        assert_eq!(named.len(), SWARM_SAMPLE);
        assert!(!named.contains(&addrs[SWARM_SAMPLE - 2].id));
        assert!(named.contains(&newcomer.addr.id));
        assert_eq!(sample.iter().filter(|m| m.trusted).count(), 4);

        // Full of trusted followers: nothing changes.
        let mut sample: Vec<SwarmMember> = addrs.iter().map(|a| member(a, true)).collect();
        give_place(&mut sample, SWARM_SAMPLE, newcomer, |m| m.trusted);
        assert!(sample.iter().all(|m| m.trusted));
    }

    #[test]
    fn the_recent_host_place_goes_to_the_host_named_least_or_to_nobody() {
        let named = NamedHosts::default();
        assert!(named.least(Vec::<(EndpointId, ())>::new()).is_none());
        let [a, b, c] = [(); 3].map(|_| some_addr().id);
        named.note(&[a, a, b]);
        let least = named.least(vec![(a, ()), (b, ()), (c, ())]);
        assert_eq!(least.map(|(id, _)| id), Some(c));
        let least = named.least(vec![(a, ()), (b, ())]);
        assert_eq!(least.map(|(id, _)| id), Some(b));
    }

    #[test]
    fn a_request_without_a_selected_path_isnt_taken_as_relayed() {
        let heard = Heard::default();
        let [relayed, new] = [(); 2].map(|_| some_addr());
        let now = Instant::now();
        heard.note(relayed.id, Some(false), now);
        // Between paths: as last heard, or directly for a new one.
        heard.note(relayed.id, None, now);
        heard.note(new.id, None, now);

        let picked = heard.pick_hosts(untrusted([&relayed, &new]), now, HEARD_LATELY);
        assert_eq!(picked.pool, [1, 1, 0]);
        assert_eq!(ids(&picked), [new.id, relayed.id]);
    }

    #[test]
    fn a_full_record_forgets_followers_not_heard_from_lately() {
        let heard = Heard::default();
        let start = Instant::now();
        let old = some_addr();
        heard.note(old.id, Some(true), start);
        for _ in 1..MAX_FOLLOWERS {
            heard.note(some_addr().id, Some(true), start + HEARD_LATELY);
        }
        assert_eq!(heard.lock().len(), MAX_FOLLOWERS);
        // Full: one more makes room by forgetting the old one.
        let new = some_addr();
        heard.note(new.id, Some(true), start + HEARD_LATELY);
        assert!(heard.lock().contains_key(&new.id));
        assert!(!heard.lock().contains_key(&old.id));
        // Full of followers heard from lately: no room, so a new one isn't
        // noted, but one already noted still is.
        let noted = *heard.lock().keys().next().unwrap();
        let soon = start + HEARD_LATELY + Duration::from_secs(1);
        heard.note(some_addr().id, Some(false), soon);
        heard.note(noted, Some(false), soon);
        assert_eq!(heard.lock().len(), MAX_FOLLOWERS);
        assert_eq!(heard.lock()[&noted], (soon, false));
    }
}
