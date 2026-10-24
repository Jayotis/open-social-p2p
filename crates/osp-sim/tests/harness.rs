//! The harness itself: a small run, measured the way a full B8 run is.
//! Requires ffmpeg.

use std::{path::Path, time::Duration};

use osp_node::{Config, Node, SimSettings};
use osp_proto::rpc::{ClientRequest, ClientResponse};
use osp_sim::Scenario;

#[tokio::test(flavor = "multi_thread")]
async fn a_small_run_measures_the_creators_upload() {
    let dir = tempfile::tempdir().unwrap();
    let scenario = Scenario {
        dir: dir.path().join("run"),
        followers: 12,
        video_bytes: 1_000_000,
        creator_mbps: 20,
        follower_mbps: vec![5, 10, 20],
        swarm: false,
        prefetch_spread: Duration::ZERO,
        time_limit: Duration::from_secs(90),
        sample: Duration::from_secs(1),
        label: "test".into(),
    };
    let report = osp_sim::run(&scenario).await.unwrap();
    println!("{}", report.summary());

    assert_eq!((report.followers, report.label.as_str()), (12, "test"));
    assert!(report.summary().contains("swarm off"));
    assert!(
        (600_000..1_400_000).contains(&report.video_bytes),
        "{}",
        report.video_bytes
    );
    assert!(report.media_bytes > report.video_bytes);

    // Today nobody but the creator serves the video, and 12 followers fit
    // within its transfer limits, so every follower gets it from the creator.
    assert_eq!(report.holding(), 12, "{}", report.summary());
    let times: Vec<f64> = report.held_after_secs.iter().flatten().copied().collect();
    assert!(times.iter().all(|t| *t <= report.ended_secs));
    assert!(report.ended_secs < 90.0, "ended as soon as all held it");
    let fastest = times.iter().copied().fold(f64::INFINITY, f64::min);
    let slowest = times.iter().copied().fold(0.0, f64::max);
    assert_eq!(report.time_until(100), Some(slowest));
    // 95% of 12 is 11.4 followers, so all 12.
    assert_eq!(report.time_until(95), Some(slowest));
    assert_eq!(report.time_until(1), Some(fastest));
    assert!(report.time_until(50).unwrap() <= slowest);

    // One copy of the media per follower, by the creator's record and by the
    // followers' records alike.
    let copies = report.creator_copies();
    assert!((11.5..13.0).contains(&copies), "{copies}");
    let received = report.received_from_creator / report.media_bytes as f64;
    assert!((copies - received).abs() < 0.1, "{copies} vs {received}");
    let d = &report.downloads;
    assert!(d.completed.round() >= 12.0, "{d:?}");
    assert_eq!((d.bad_data, d.failed), (0.0, 0.0), "{d:?}");

    // Noted once a second, never more than the final count. Counts fade a
    // little between readings, so they're compared with a margin.
    let so_far = &report.creator_sent_so_far;
    assert!(!so_far.is_empty());
    assert!(
        so_far.windows(2).all(|w| w[0] <= w[1] * 1.0001),
        "{so_far:?}"
    );
    assert!(*so_far.last().unwrap() <= report.creator_sent_bytes * 1.0001);
    assert_eq!(report.follower_sent_bytes.len(), 12);

    // 12 followers allow the creator at most 1.2 copies.
    let marks = report.marks();
    assert!(!marks.creator_copies, "{marks:?}");
    assert!(
        marks.most_within_15_min && marks.all_within_30_min,
        "{marks:?}"
    );

    // The homes are on disk until the caller removes them.
    assert!(scenario.dir.join("creator").exists());
    let err = osp_sim::run(&scenario).await.unwrap_err();
    assert!(err.to_string().contains("isn't empty"), "{err:#}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_small_run_with_the_swarm_on() {
    let dir = tempfile::tempdir().unwrap();
    // Over 1 MiB, so the followers fetch it through the swarm.
    let scenario = Scenario {
        dir: dir.path().join("run"),
        followers: 12,
        video_bytes: 2_000_000,
        creator_mbps: 20,
        follower_mbps: vec![5, 10, 20],
        swarm: true,
        prefetch_spread: Duration::from_secs(10),
        time_limit: Duration::from_secs(90),
        sample: Duration::from_secs(1),
        label: "test".into(),
    };
    let report = osp_sim::run(&scenario).await.unwrap();
    println!("{}", report.summary());
    assert!(
        report
            .summary()
            .contains("swarm on, fetches spread over 10 s")
    );
    assert_eq!(report.holding(), 12, "{}", report.summary());

    // Followers fetching later got it from followers fetching earlier.
    let video = report.video_bytes as f64;
    let sharing = report.follower_sent_bytes.iter().filter(|b| **b >= video);
    assert!(sharing.count() >= 1, "{}", report.summary());
    assert!(report.creator_copies() < 11.0, "{}", report.summary());
}

async fn start(root: &Path, name: &str, sim: Option<SimSettings>) -> Node {
    let home = root.join(name);
    Node::init(&home, &Config::local_test(name)).unwrap();
    match sim {
        Some(sim) => Node::spawn_sim(&home, sim).await.unwrap(),
        None => Node::spawn(&home).await.unwrap(),
    }
}

async fn ticket(node: &Node) -> String {
    match node.handle(ClientRequest::Invite).await {
        ClientResponse::Ticket { ticket } => ticket,
        other => panic!("unexpected {other:?}"),
    }
}

async fn follower_count(node: &Node) -> usize {
    match node.handle(ClientRequest::Status).await {
        ClientResponse::Status(status) => status.followers,
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn spawn_sim_changes_the_new_follower_limit() {
    let dir = tempfile::tempdir().unwrap();
    let sim = SimSettings {
        new_followers_per_hour: 2,
        ..SimSettings::default()
    };
    let limited = start(dir.path(), "limited", Some(sim)).await;
    let usual = start(dir.path(), "usual", None).await;
    let tickets = [ticket(&limited).await, ticket(&usual).await];

    let mut followers = Vec::new();
    for i in 0..4 {
        let node = start(dir.path(), &format!("f{i}"), None).await;
        for ticket in &tickets {
            let resp = node
                .handle(ClientRequest::Follow {
                    ticket: ticket.clone(),
                })
                .await;
            assert!(!matches!(resp, ClientResponse::Error { .. }), "{resp:?}");
        }
        followers.push(node);
    }

    // Everyone has subscribed once the usual node has all four.
    for _ in 0..100 {
        if follower_count(&usual).await == 4 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(follower_count(&usual).await, 4);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        follower_count(&limited).await,
        2,
        "the limit given was kept"
    );

    for node in followers.into_iter().chain([limited, usual]) {
        node.shutdown().await.unwrap();
    }
}
