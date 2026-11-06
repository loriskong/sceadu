use sceadu_core::session::file::{sha256_file, FileStore};
use sceadu_core::{spawn_with_config, Identity, Multiaddr, NodeConfig};

#[tokio::test]
async fn transfers_a_file_to_the_receiver_store() {
    let a_identity = Identity::generate();
    let b_identity = Identity::generate();
    let a_store = tempfile::tempdir().unwrap();
    let b_store = tempfile::tempdir().unwrap();
    let source_dir = tempfile::tempdir().unwrap();
    let source = source_dir.path().join("court.jpg");
    let contents = vec![0x5a; 2 * 1024 * 1024 + 17];
    std::fs::write(&source, &contents).unwrap();

    let (b, b_addr) = spawn_with_config(
        &b_identity,
        Some("/ip6/::1/tcp/0".parse::<Multiaddr>().unwrap()),
        NodeConfig {
            file_store_path: Some(b_store.path().to_path_buf()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let b_addr = b_addr.unwrap();
    let (a, _) = spawn_with_config(
        &a_identity,
        None,
        NodeConfig {
            file_store_path: Some(a_store.path().to_path_buf()),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let peer = a.dial(b_addr).await.unwrap();
    let file = a.send_file(peer, &source).await.unwrap();
    let hash = sha256_file(&source).unwrap();
    assert_eq!(file.sha256, hash);
    assert_eq!(file.size, contents.len() as u64);

    let target = b_store.path().join(hash);
    for _ in 0..50 {
        if target.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(target.exists());
    assert_eq!(std::fs::read(&target).unwrap(), contents);

    a.shutdown().await;
    b.shutdown().await;
    let _ = FileStore::open(a_store.path());
}
