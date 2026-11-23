//! A circuit end's inbox, in a binary of its own: the test floods loopback,
//! and the circuits tests beside it lose packets they don't send again.

mod common;

use std::time::Duration;

use common::*;
use osp_carrier::{ALPN, HostEvent, Limits, ViewerCircuit, inbox_drops};
use tokio::time::timeout;

/// Alice's node sends Jimmy's circuit more packets than its inbox holds,
/// and Jimmy's end reads none of them until the last is sent. Each packet
/// his link brought in is either read or counted as dropped at the full
/// inbox: none goes unseen (stage D's sixth run broke on losses nothing
/// counted). Not every packet sent need reach his link: on a busy machine
/// some are lost on the way, where QUIC doesn't send datagrams again.
#[tokio::test]
async fn a_full_viewer_inbox_counts_each_packet_it_drops() {
    let book = Book::default();
    let p = carrier(&book, Limits::home()).await;
    let q = carrier(&book, Limits::home()).await;
    let (_h, mut host) = host(&q).await;
    let endpoint = endpoint(vec![]).await;
    let hops = [p.descriptor().await, q.descriptor().await];
    let conn = endpoint.connect(p.endpoint.addr(), ALPN).await.unwrap();
    let link = conn.clone();
    let mut circuit = ViewerCircuit::build(conn, &hops, host.slot())
        .await
        .unwrap();
    let opened = match timeout(WAIT, host.recv()).await.unwrap().unwrap() {
        HostEvent::Opened(id) => id,
        other => panic!("expected the circuit to open, got {other:?}"),
    };
    // What the link brought in to build the circuit: `CREATED` and the
    // answers to `EXTEND` and `CONNECT_HOST`.
    let setup = link.stats().frame_rx.datagram;
    assert_eq!(setup, 3);

    let before = inbox_drops().viewer;
    const SENT: u64 = 3000;
    for i in 0..SENT {
        host.send(opened, vec![i as u8; 1000]).unwrap();
        if i % 10 == 9 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
    // Read until nothing more comes for a while.
    let mut kept = 0;
    while let Ok(Some(_)) = timeout(Duration::from_secs(1), circuit.recv()).await {
        kept += 1;
    }
    // Other tests in this binary only add to the count, and none of them
    // fills an inbox.
    let dropped = inbox_drops().viewer - before;
    let came = link.stats().frame_rx.datagram - setup;
    assert_eq!(
        kept + dropped,
        came,
        "{kept} read and {dropped} counted, of {came} the link brought in ({SENT} sent)"
    );
    // The inbox holds 1024, and as many again wait to be unwrapped.
    if came > 2 * 1024 {
        assert!(dropped > 0, "{came} came, {kept} read, none counted");
    }
}
