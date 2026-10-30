//! What a node measures about the peer nodes it deals with (ADR 0011).
//!
//! The record is private: it never leaves the node, and nothing here
//! changes how the node behaves yet. It is kept per node ID, as counts
//! that fade by half every 14 days, so old behaviour stops counting.

use std::time::Duration;

use iroh::EndpointId;
use osp_proto::rpc::PeerCounts;
use tracing::warn;

use crate::{
    db::{Db, now_ms},
    store::Outcome,
};

/// Counts halve in this long.
const HALF_LIFE: Duration = Duration::from_secs(14 * 24 * 60 * 60);
/// Most rows kept in each group: nodes this node contacted, and nodes that
/// only contacted it. Kept apart so a crowd of strangers, whose node IDs
/// cost nothing, can't push out the record of the nodes this node relies on.
pub(crate) const MAX_PER_GROUP: usize = 1000;
/// A peer not in contact for this long is forgotten.
pub(crate) const FORGET_AFTER: Duration = Duration::from_secs(90 * 24 * 60 * 60);

/// Something that happened with a peer.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Event {
    /// This node sent it a request. `reached` is false if it couldn't
    /// connect; `took` is how long the reply took, if one came.
    Request {
        reached: bool,
        took: Option<Duration>,
    },
    /// This node tried to download (part of) a blob from it.
    Download {
        outcome: Outcome,
        bytes: u64,
        took: Duration,
        first_byte: Option<Duration>,
    },
    /// This node sent it media.
    Served { bytes: u64 },
    /// This node refused it media because it was busy.
    RefusedBusy,
}

impl Event {
    /// Whether this node made the contact.
    fn ours(&self) -> bool {
        matches!(self, Event::Request { .. } | Event::Download { .. })
    }
}

/// Adds `event` to what this node knows of `node`. A failure is only
/// logged: measuring must never get in the way of what it measures.
pub(crate) fn record(db: &Db, node: &EndpointId, event: Event) {
    let result = db.update_peer(node, event.ours(), now_ms(), |counts| add(counts, event));
    if let Err(e) = result {
        warn!(node = %node.fmt_short(), "recording a peer measurement failed: {e:#}");
    }
}

/// Counts as they are `elapsed_ms` after they were last updated.
pub(crate) fn fade(counts: PeerCounts, elapsed_ms: i64) -> PeerCounts {
    let half_lives = elapsed_ms.max(0) as f64 / HALF_LIFE.as_millis() as f64;
    counts.scaled(0.5f64.powf(half_lives))
}

fn add(c: &mut PeerCounts, event: Event) {
    match event {
        Event::Request { reached, took } => {
            c.tries += 1.0;
            c.requests += 1.0;
            if !reached {
                c.unreached += 1.0;
            }
            if let Some(took) = took {
                c.answered += 1.0;
                c.answer_ms += ms(took);
            }
        }
        Event::Download {
            outcome,
            bytes,
            took,
            first_byte,
        } => {
            c.tries += 1.0;
            match outcome {
                Outcome::Completed => c.completed += 1.0,
                Outcome::Busy => c.busy += 1.0,
                Outcome::NotHosting => c.not_hosting += 1.0,
                Outcome::BadData => c.bad_data += 1.0,
                Outcome::Unreachable => c.unreached += 1.0,
                Outcome::Failed => c.failed += 1.0,
                Outcome::HandedOn => c.handed_on += 1.0,
                // We stopped it; that says nothing about the node.
                Outcome::Stopped => {}
            }
            if bytes > 0 {
                c.received_bytes += bytes as f64;
                c.receive_ms += ms(took);
            }
            if let Some(first_byte) = first_byte {
                c.first_byte_ms += ms(first_byte);
                c.first_bytes += 1.0;
            }
        }
        Event::Served { bytes } => {
            c.transfers += 1.0;
            c.sent_bytes += bytes as f64;
        }
        Event::RefusedBusy => c.refused_busy += 1.0,
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY_MS: i64 = 24 * 60 * 60 * 1000;

    #[test]
    fn counts_halve_every_fourteen_days_and_averages_fade_with_them() {
        let mut counts = PeerCounts::default();
        for took in [100, 300] {
            add(
                &mut counts,
                Event::Request {
                    reached: true,
                    took: Some(Duration::from_millis(took)),
                },
            );
        }
        let later = fade(counts.clone(), 14 * DAY_MS);
        assert!((later.answered - 1.0).abs() < 1e-9, "{later:?}");
        // The average reply time stays what it was.
        assert!((later.answer_ms / later.answered - 200.0).abs() < 1e-6);
        let much_later = fade(counts.clone(), 28 * DAY_MS);
        assert!((much_later.requests - 0.5).abs() < 1e-9, "{much_later:?}");
        assert_eq!(fade(counts.clone(), 0), counts);
        // A clock that went back doesn't make counts grow.
        assert_eq!(fade(counts.clone(), -DAY_MS), counts);
    }

    #[test]
    fn each_event_counts_where_it_belongs() {
        let mut c = PeerCounts::default();
        add(
            &mut c,
            Event::Request {
                reached: false,
                took: None,
            },
        );
        add(
            &mut c,
            Event::Request {
                reached: true,
                took: None,
            },
        );
        assert_eq!(
            (c.tries, c.requests, c.unreached, c.answered),
            (2.0, 2.0, 1.0, 0.0)
        );

        let download = |outcome, bytes| Event::Download {
            outcome,
            bytes,
            took: Duration::from_millis(500),
            first_byte: (bytes > 0).then_some(Duration::from_millis(20)),
        };
        for outcome in [
            Outcome::Completed,
            Outcome::Busy,
            Outcome::NotHosting,
            Outcome::BadData,
            Outcome::Unreachable,
            Outcome::Failed,
            Outcome::HandedOn,
            Outcome::Stopped,
        ] {
            add(&mut c, download(outcome, 0));
        }
        assert_eq!(c.tries, 10.0);
        assert_eq!(c.unreached, 2.0);
        assert_eq!(
            (
                c.completed,
                c.busy,
                c.not_hosting,
                c.bad_data,
                c.failed,
                c.handed_on
            ),
            (1.0, 1.0, 1.0, 1.0, 1.0, 1.0)
        );
        // Only attempts that brought bytes count toward speed.
        assert_eq!(
            (c.received_bytes, c.receive_ms, c.first_bytes),
            (0.0, 0.0, 0.0)
        );
        add(&mut c, download(Outcome::Failed, 1000));
        assert_eq!((c.received_bytes, c.receive_ms), (1000.0, 500.0));
        assert_eq!((c.first_byte_ms, c.first_bytes), (20.0, 1.0));

        add(&mut c, Event::Served { bytes: 4096 });
        add(&mut c, Event::RefusedBusy);
        assert_eq!(
            (c.transfers, c.sent_bytes, c.refused_busy),
            (1.0, 4096.0, 1.0)
        );
        assert!(
            Event::Download {
                outcome: Outcome::Busy,
                bytes: 0,
                took: Duration::ZERO,
                first_byte: None
            }
            .ours()
        );
        assert!(!Event::Served { bytes: 1 }.ours());
        assert!(!Event::RefusedBusy.ours());
    }
}
