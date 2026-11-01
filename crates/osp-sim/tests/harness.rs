//! The harness itself: a small run, measured the way a full B8 run is.
//! Requires ffmpeg.

use std::{path::Path, time::Duration};

use osp_node::DirectRule;
use osp_node::{Config, Node, SimSettings};
use osp_proto::rpc::{ClientRequest, ClientResponse};
use osp_sim::{Network, Scenario};

#[tokio::test(flavor = "multi_thread")]
async fn a_small_run_measures_the_creators_upload() {
    let dir = tempfile::tempdir().unwrap();
    let scenario = Scenario {
        dir: dir.path().join("run"),
        followers: 12,
        video_bytes: 1_000_000,
        creator_mbps: 20,
        follower_mbps: vec![5, 10, 20],
        network: None,
        swarm: false,
        prefetch_spread: Duration::ZERO,
        download_sources: 3,
        drop_out: 0,
        drop_out_after: Duration::ZERO,
        strangers: 0,
        latecomers: 0,
        latecomers_after: Duration::ZERO,
        time_limit: Duration::from_secs(90),
        sample: Duration::from_secs(1),
        trust_mbps: Vec::new(),
        tor_delay: false,
        endpoints: None,
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

/// A small run with the swarm on: 12 followers spreading their fetches of
/// a video over 10 seconds.
fn with_the_swarm(dir: &Path) -> Scenario {
    Scenario {
        dir: dir.join("run"),
        followers: 12,
        // Over 1 MiB, so the followers fetch it through the swarm.
        video_bytes: 2_000_000,
        creator_mbps: 20,
        follower_mbps: vec![5, 10, 20],
        network: None,
        swarm: true,
        prefetch_spread: Duration::from_secs(10),
        download_sources: 3,
        drop_out: 0,
        drop_out_after: Duration::ZERO,
        strangers: 0,
        latecomers: 0,
        latecomers_after: Duration::ZERO,
        time_limit: Duration::from_secs(90),
        sample: Duration::from_secs(1),
        trust_mbps: Vec::new(),
        tor_delay: false,
        endpoints: None,
        label: "test".into(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_small_run_with_the_swarm_on() {
    let dir = tempfile::tempdir().unwrap();
    let scenario = Scenario {
        trust_mbps: vec![5],
        tor_delay: false,
        endpoints: None,
        ..with_the_swarm(dir.path())
    };
    let report = osp_sim::run(&scenario).await.unwrap();
    println!("{}", report.summary());
    assert!(
        report
            .summary()
            .contains("swarm on, fetches spread over 10 s")
    );
    // A follower in three uploads at 5 Mbit/s, and asked to be trusted.
    assert!(
        report
            .summary()
            .contains("trusted: 4 followers, those uploading at 5 Mbit/s")
    );
    assert_eq!(report.holding(), 12, "{}", report.summary());

    // Followers fetching later got it from followers fetching earlier, a
    // copy at least between them: in parts, it may come from several.
    let video = report.video_bytes as f64;
    let sent: f64 = report.follower_sent_bytes.iter().sum();
    assert!(sent >= video, "{}", report.summary());
    assert!(report.creator_copies() < 11.0, "{}", report.summary());
}

#[tokio::test(flavor = "multi_thread")]
async fn latecomers_fetch_from_several_followers_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let scenario = Scenario {
        // Over 2 MiB, so it comes in parts.
        video_bytes: 3_000_000,
        latecomers: 2,
        // Later than the run ends, about 20 s after posting.
        latecomers_after: Duration::from_secs(30),
        ..with_the_swarm(dir.path())
    };
    let report = osp_sim::run(&scenario).await.unwrap();
    let summary = report.summary();
    println!("{summary}");
    assert!(
        summary.contains("each file from up to 3 nodes at once"),
        "{summary}"
    );
    assert!(summary.contains("2 of 2 held the video"), "{summary}");
    assert!(report.latecomers_started_secs >= 30.0, "{summary}");
    assert!(report.latecomer_time(100).is_some(), "{summary}");
    // Each got the thumbnail from the creator, and the video in two parts
    // or more from followers holding a copy.
    let d = &report.latecomer_downloads;
    assert!(d.completed.round() >= 6.0, "{summary}");
    let thumbnails = (report.media_bytes - report.video_bytes) as f64;
    assert!(
        report.creator_sent_to_latecomers < 2.0 * thumbnails + 1000.0,
        "{summary}"
    );
    // They aren't counted with the followers.
    assert_eq!(report.held_after_secs.len(), 12);
    assert_eq!(report.holding(), 12, "{summary}");
}

#[tokio::test(flavor = "multi_thread")]
async fn followers_that_stop_are_left_out_of_the_times() {
    let dir = tempfile::tempdir().unwrap();
    let scenario = Scenario {
        drop_out: 3,
        drop_out_after: Duration::from_secs(8),
        time_limit: Duration::from_secs(120),
        ..with_the_swarm(dir.path())
    };
    let report = osp_sim::run(&scenario).await.unwrap();
    let summary = report.summary();
    println!("{summary}");

    // The last of each group of four, so every upload speed.
    assert_eq!(report.dropped_out, [3, 7, 11]);
    assert!(summary.contains("3 followers stopped at 8 s"), "{summary}");
    // The 9 that stayed got the video anyway, and only they are counted.
    assert_eq!(report.holding(), 9, "{summary}");
    assert!(summary.contains("held the video: 9 of 9"), "{summary}");
    assert!(report.ended_secs < 120.0, "ended once those 9 held it");
    let stayed: Vec<f64> = [0, 1, 2, 4, 5, 6, 8, 9, 10]
        .iter()
        .map(|i| report.held_after_secs[*i].expect("held"))
        .collect();
    let slowest = stayed.iter().copied().fold(0.0, f64::max);
    assert_eq!(report.time_until(100), Some(slowest));
    // What the stopped followers measured is kept.
    assert_eq!(report.follower_sent_bytes.len(), 12);
    let copies = report.received_from_creator / report.media_bytes as f64;
    assert!(copies >= 1.0, "{summary}");
}

#[tokio::test(flavor = "multi_thread")]
async fn strangers_get_a_reshared_video_from_each_other() {
    let dir = tempfile::tempdir().unwrap();
    // Spread over long enough that the first strangers hold a copy before
    // the last ask who has one.
    let scenario = Scenario {
        strangers: 12,
        prefetch_spread: Duration::from_secs(30),
        time_limit: Duration::from_secs(180),
        ..with_the_swarm(dir.path())
    };
    let report = osp_sim::run(&scenario).await.unwrap();
    let summary = report.summary();
    println!("{summary}");

    // The first follower reshared as soon as it held the video, and every
    // stranger got it after that.
    let reshared = report.reshared_after_secs.expect("reshared");
    assert!(reshared >= report.held_after_secs[0].unwrap(), "{summary}");
    assert_eq!(report.stranger_held_after_secs.len(), 12);
    for held in &report.stranger_held_after_secs {
        assert!(held.is_some_and(|t| t >= reshared), "{summary}");
    }
    assert!(
        summary.contains("12 of 12 strangers held the video"),
        "{summary}"
    );
    assert_eq!(report.holding(), 12, "{summary}");

    // The resharer hosts the video, so it names strangers holding a copy
    // to the others, and strangers fetching later got it from them.
    let video = report.video_bytes as f64;
    let sent: f64 = report.stranger_sent_bytes.iter().sum();
    assert!(sent >= video, "{summary}");
    // What the creator sent them, if anything, is counted the same on both
    // sides.
    let copies = |bytes: f64| bytes / report.media_bytes as f64;
    let to_strangers = copies(report.creator_sent_to_strangers);
    let by_strangers = copies(report.strangers_received_from_creator);
    assert!(
        (to_strangers - by_strangers).abs() < 0.1,
        "{to_strangers} vs {by_strangers}"
    );
    let received = copies(report.received_from_creator) + by_strangers;
    assert!(
        (report.creator_copies() - received).abs() < 0.1,
        "{summary}"
    );
}

/// B8's swarm in small, as nodes will run it (ADR 0014): messages taking as
/// long as they do over Tor, media through each node's media endpoint, and
/// what's measured counted against each node's public ID.
#[tokio::test(flavor = "multi_thread")]
async fn a_small_run_with_tor_like_messages_and_media_on_its_own_endpoints() {
    let dir = tempfile::tempdir().unwrap();
    let scenario = Scenario {
        video_bytes: 3_000_000,
        network: Some(Network {
            creator_down_mbps: 100,
            creator_delay_ms: 20,
            creator_loss_percent: 0.0,
            down_mbps: vec![50, 100, 300],
            delay_ms: vec![10, 20, 40],
            loss_percent: vec![0.0],
            caps: false,
            // Cut off from 4 s and 8 s, for 3 s in every 12.
            flaky: 2,
            outage_secs: 3,
            outage_every_secs: 12,
            seed: 1,
        }),
        latecomers: 1,
        tor_delay: true,
        endpoints: Some(DirectRule::Nobody),
        prefetch_spread: Duration::from_secs(30),
        time_limit: Duration::from_secs(420),
        ..with_the_swarm(dir.path())
    };
    let report = osp_sim::run(&scenario).await.unwrap();
    let summary = report.summary();
    println!("{summary}");
    assert!(summary.contains("media and direct endpoints"), "{summary}");
    assert_eq!(report.holding(), 12, "{summary}");
    // By public ID: what followers received from the creator's media endpoint
    // is what it sent to them, as measured at each end.
    let copies = |bytes: f64| bytes / report.media_bytes as f64;
    assert!(
        (report.creator_copies() - copies(report.received_from_creator)).abs() < 0.1,
        "{summary}"
    );
    assert!(report.creator_copies() < 12.5, "{summary}");
    assert!(report.latecomer_time(100).is_some(), "{summary}");
    assert!(report.network.as_ref().unwrap().outages >= 2, "{summary}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_small_run_over_a_simulated_network() {
    let dir = tempfile::tempdir().unwrap();
    let scenario = Scenario {
        // Over 2 MiB, so it comes in parts.
        video_bytes: 3_000_000,
        network: Some(Network {
            creator_down_mbps: 100,
            creator_delay_ms: 20,
            creator_loss_percent: 0.5,
            down_mbps: vec![50, 100, 300],
            delay_ms: vec![10, 20, 40],
            loss_percent: vec![0.0, 1.0],
            caps: false,
            // Cut off from 4 s and 8 s, for 3 s in every 12.
            flaky: 2,
            outage_secs: 3,
            outage_every_secs: 12,
            seed: 1,
        }),
        latecomers: 1,
        // Without upload caps, a slow connection's upload is shared by all
        // its transfers at once, so the first copies take longer than on
        // loopback: spread long enough that early ones still serve later.
        prefetch_spread: Duration::from_secs(30),
        // Usually under a minute; room for a slow machine.
        time_limit: Duration::from_secs(300),
        ..with_the_swarm(dir.path())
    };
    let report = osp_sim::run(&scenario).await.unwrap();
    let summary = report.summary();
    println!("{summary}");
    assert!(
        summary.contains("simulated network, speeds above up: creator 100 Mbit/s down, 20 ms each way, 0.5% loss; followers 50/100/300 Mbit/s down, 10/20/40 ms each way, 0/1% loss; no upload caps"),
        "{summary}"
    );
    assert!(
        summary.contains("2 followers cut off for 3 s in every 12 s"),
        "{summary}"
    );

    // Everyone got the video, the flaky followers too. How much came from
    // the creator depends on timing; the latecomer, fetching once everyone
    // held it, got it from followers, in parts.
    let net = report.network.as_ref().expect("a network");
    assert_eq!(net.flaky_followers, [2, 8]);
    // Each was cut off at least once before everyone held the video.
    assert!(net.outages >= 2, "{summary}");
    assert_eq!(report.holding(), 12, "{summary}");
    assert!(report.creator_copies() < 12.5, "{summary}");
    assert!(report.latecomer_time(100).is_some(), "{summary}");
    let thumbnail = (report.media_bytes - report.video_bytes) as f64;
    assert!(
        report.creator_sent_to_latecomers < thumbnail + 1000.0,
        "{summary}"
    );
    assert!(report.latecomer_downloads.completed.round() >= 3.0);
    // Packets were lost along the way, and QUIC sent them again.
    let p = &net.packets;
    assert!(p.lost > 0 && p.delivered > 0, "{p:?}");
    // And some while the flaky followers were cut off.
    assert!(p.cut_off > 0, "{p:?}");
    assert!(p.delivered + p.lost + p.queue_full + p.cut_off <= p.packets);
    assert_eq!(report.downloads.bad_data, 0.0, "{summary}");
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
