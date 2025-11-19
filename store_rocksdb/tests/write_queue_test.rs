use std::{sync::mpsc, thread, time::Duration};

use store_rocksdb::WriteQueue;
use store_traits::ledger::{WriteStrategy, WriterType};

#[test]
fn optimistic_writers_can_share() {
    let queue = WriteQueue::new();
    let _g1 = queue.request_write(WriterType::Testing, WriteStrategy::Optimistic);
    let _g2 = queue.request_write(WriterType::Testing, WriteStrategy::Optimistic);

    let stats = queue.stats();
    assert_eq!(stats.optimistic_holders, 2);
    assert!(!stats.pessimistic_active);
}

#[test]
fn pessimistic_writer_blocks_optimistic() {
    let queue = WriteQueue::new();
    let pess = queue.request_write(WriterType::Testing, WriteStrategy::Pessimistic);

    let (tx, rx) = mpsc::channel();
    let queue_clone = queue.clone();
    thread::spawn(move || {
        let guard = queue_clone.request_write(WriterType::Testing, WriteStrategy::Optimistic);
        let _ = tx.send(());
        drop(guard);
    });

    assert!(rx.recv_timeout(Duration::from_millis(50)).is_err());
    drop(pess);
    rx.recv_timeout(Duration::from_secs(1))
        .expect("optimistic writer should acquire after pessimistic release");
}
