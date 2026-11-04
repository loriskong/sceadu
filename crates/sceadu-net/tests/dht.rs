use std::time::Duration;

use sceadu_core::Identity;
use sceadu_net::{spawn, Multiaddr};

#[tokio::test]
async fn dht_publishes_and_fetches_manifest() {
    let publisher_identity = Identity::generate();
    let reader_identity = Identity::generate();

    let (publisher, publisher_addr) = spawn(
        &publisher_identity,
        Some("/ip6/::1/tcp/0".parse::<Multiaddr>().unwrap()),
    )
    .await
    .expect("publisher starts");
    let publisher_addr = publisher_addr.expect("publisher listens");

    let (reader, reader_addr) = spawn(
        &reader_identity,
        Some("/ip6/::1/tcp/0".parse::<Multiaddr>().unwrap()),
    )
    .await
    .expect("reader starts");
    let reader_addr = reader_addr.expect("reader listens");

    let reader_peer = publisher
        .dial(reader_addr.clone())
        .await
        .expect("publisher dials reader");
    publisher
        .add_address(reader_peer, reader_addr)
        .await
        .expect("publisher records reader address");
    reader
        .add_address(publisher.peer_id(), publisher_addr)
        .await
        .expect("reader records publisher address");

    tokio::time::sleep(Duration::from_millis(250)).await;
    publisher.bootstrap().await.expect("publisher bootstraps");
    reader.bootstrap().await.expect("reader bootstraps");
    tokio::time::sleep(Duration::from_millis(100)).await;

    let mut published = false;
    for _ in 0..20 {
        if publisher.publish().await.is_ok() {
            published = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(published, "publish succeeds");

    let mut fetched = None;
    for _ in 0..20 {
        match reader.lookup(publisher_identity.agent_id()).await {
            Ok(manifest) => {
                fetched = Some(manifest);
                break;
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    }

    let manifest = fetched.expect("reader finds the published manifest");
    assert_eq!(&manifest.body.agent_id, &publisher_identity.agent_id());

    reader.shutdown().await;
    publisher.shutdown().await;
}
