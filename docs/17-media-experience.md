# Media Experience

**Status:** Post-Product-Alpha product and implementation plan. This document describes the future continuous media experience, including short-form video. It does not create a new protocol by itself and does not change the current Product Alpha shell.

Media should feel immediate on the phone while preserving OSP's architecture: the phone is a viewer/editor for its own home node, and the node discovers, ranks, prepares and retrieves content in the background.

See [doc 15](15-feature-responsibility-model.md) and the open-feed requirements in [doc 08](08-open-feed-algorithm.md).

## Product shape

A future **Media** destination opens directly into a full-screen media viewer.

Initial experience:

- vertical full-screen video for short-form posts;
- swipe to the next item;
- creator/profile, caption and community/context where applicable;
- Like, Reply, Share and Follow;
- Following and Discover modes;
- clear "Why am I seeing this?" explanation;
- controls for topics, languages, mutes and recommendation sources;
- graceful unavailable/buffering states.

Media is another view of ordinary OSP posts and creators. It does not create a separate video identity, follower graph or "Media account".

Longer video and richer creator/channel views can use the same foundation later.

## App work

The phone owns:

- full-screen player;
- gesture/navigation behaviour;
- playback controls, captions and accessibility;
- camera/microphone capture;
- draft editing, trimming and preview;
- posting progress;
- interaction buttons and replies;
- quick preference controls such as "less like this";
- clear labels for sponsored material if OSP ever adds it;
- offline/cached display.

The phone should not query remote creators or peers directly to fill the next swipe. It asks its node for the queue and media URLs/streams it can consume.

Editing should remain phone-friendly. Expensive transcoding and network distribution stay on the node.

## Client API work

Provisional intent-level calls may include:

- `mediaFeed(mode, cursor)`
- `mediaNext(after)`
- `mediaOpened(post)`
- `mediaFinished(post, progress)`
- `mediaPreference(post/topic, action)`
- existing Like/Reply/Follow/Share calls
- existing/resumable upload calls for creation

The exact amount of watch-state sent to the node is a privacy/product decision. It should be the minimum needed to produce the behaviour the owner selected.

Playback continues through the existing app-local media proxy/Rust client path rather than exposing arbitrary peer endpoints to Flutter.

## Home-node work

The node creates the Media experience:

- build candidate sets from follows, public discovery, communities and Hunters;
- apply blocks, moderation exclusions, topic/language controls and the selected feed mode;
- rank locally with a published algorithm/version;
- diversify creators/topics so one source does not dominate endlessly;
- explain the main reason each candidate was selected;
- prepare thumbnails/metadata;
- opportunistically prefetch media or initial ranges within explicit bandwidth/storage limits;
- retrieve from verified replicas before overloading the creator where the existing swarm supports it;
- maintain a bounded local cache;
- transcode the owner's uploads;
- continue discovery/prefetch while the phone is off when the owner permits it;
- run optional personal moderation filters before candidates reach the app.

Viewing history and detailed preferences stay on the owner's node where feasible. They are not published as recommendation signals merely because the feed uses them locally.

## Protocol work

The first Media implementation should reuse existing OSP posts, profiles, attachments, discovery, viewing endpoints and media distribution as far as possible.

A new network protocol is needed only for facts independent nodes must exchange. Possible future needs include:

- a better resumable/streaming manifest for longer video;
- public topic metadata;
- distributed recommendation/trending evidence;
- creator/channel metadata beyond today's profile/post model.

A vertical viewer itself is **not** a protocol reason.

Network health and hosting performance are never organic ranking authority. A fast node or large replica count may improve availability but does not make a video more worthy of recommendation.

## Queue construction

The node should maintain a small prepared queue rather than choose only after each swipe.

A candidate goes through:

1. visibility and signature checks;
2. author/community moderation;
3. owner's blocks/mutes/content preferences;
4. feed-mode eligibility;
5. local ranking;
6. diversity/exploration;
7. availability preparation.

The node may prepare thumbnails and initial media ranges ahead of time, but must have explicit caps on:

- bytes per hour/day;
- disk used by Media cache;
- simultaneous retrieval;
- cellular/relay cost where relevant;
- speculative fetching from creators.

Automatic prefetch is a performance feature, not an engagement signal.

## Creation path

A creator records/edits on the phone, then sends the draft/files to their own node.

The node:

- reserves space;
- receives/resumes upload;
- strips/transcodes according to OSP media rules;
- creates the signed post;
- publishes it;
- announces/synchronizes normally;
- participates in the existing follower/resharer distribution system.

The phone should be able to close or lock while the durable node-side publishing job continues where platform rules allow.

## Moderation and safety

Media applies moderation before ranking/presentation.

The owner's node may:

- suppress repeated spam;
- apply personal topic/content filters;
- blur or hold locally classified sensitive material;
- use optional local moderation assistants under [doc 19](19-node-assisted-moderation.md).

Community moderation applies where a media post is being viewed in a community context. Personal filtering remains local.

No AI classifier becomes a public authority merely because it runs on a powerful node.

## Measurement

Media quality should be measured by more than watch time.

Track in tests, locally or in privacy-preserving aggregate research where explicitly designed:

- start latency;
- buffering/stalls;
- queue exhaustion;
- creator upload cost;
- bytes fetched speculatively but never viewed;
- source diversity;
- new-creator exposure;
- repeated-content rate;
- ability to understand why content was shown;
- user-reported usefulness;
- effectiveness of explicit controls.

Do not optimize a hidden global objective that requires uploading everyone's detailed viewing history.

## Build order

### MDA1 — Viewer over existing posts

- full-screen player;
- node returns eligible video posts;
- Following/New ordering first;
- current media streaming path;
- Like/Reply/Follow/Share.

### MDA2 — Prepared queue

- bounded node-side queue;
- thumbnails/initial-range preparation;
- fallback/retry under creator/peer outage;
- cache limits and diagnostics.

### MDA3 — Discover

- transparent local recommendation pipeline;
- topic/language controls;
- diversity/exploration;
- Why am I seeing this?;
- optional Hunter/community sources.

### MDA4 — Creator workflow

- phone capture/edit polish;
- background/resumable node upload;
- creator profiles/channels where needed;
- performance tests on Home and Creator node profiles.

### MDA5 — Abuse and scale

- duplicate/spam floods;
- malicious recommendation evidence;
- constrained home upload;
- relay-heavy viewing;
- thousands of candidates with bounded node CPU/storage;
- moderation-assistance tests.

## Gate

Media is ready for broader testing when a person can swipe smoothly through a queue supplied by their own node, understand/control where recommendations come from, publish from the phone without becoming a peer, and continue using OSP when creators or individual replica hosts go offline.

The gate also requires measured limits showing that speculative prefetch does not create unreasonable home bandwidth/storage costs.
