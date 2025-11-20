use std::{
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use store_rocksdb::WriteQueue;
use store_traits::ledger::{WriteStrategy, WriterType};

#[test]
fn optimistic_writers_can_share() {
    let queue = WriteQueue::new();
    let _g1 = queue.request_write(WriterType::Testing, WriteStrategy::Optimistic);
    let _g2 = queue.request_write(WriterType::Testing, WriteStrategy::Optimistic);

    let stats = queue.stats();
    assert_eq!(stats.optimistic_active, 2);
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

#[test]
fn stats_capture_waiting_depth() {
    let queue = WriteQueue::new();
    let pess = queue.request_write(WriterType::Testing, WriteStrategy::Pessimistic);

    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let queue_clone = queue.clone();
    let _handle = thread::spawn(move || {
        let guard = queue_clone.request_write(WriterType::Testing, WriteStrategy::Optimistic);
        entered_tx.send(()).unwrap();
        release_rx.recv().unwrap();
        drop(guard);
    });

    // Wait for the optimistic writer to be queued behind the pessimistic holder.
    let start = Instant::now();
    loop {
        let stats = queue.stats();
        if stats.waiting_optimistic == 1 && stats.queue_depth() == 1 {
            break;
        }
        if start.elapsed() > Duration::from_millis(250) {
            panic!("optimistic writer never queued");
        }
        thread::sleep(Duration::from_millis(5));
    }

    // Allow the optimistic writer to acquire and finish.
    release_tx.send(()).unwrap();
    drop(pess);
    entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
}
