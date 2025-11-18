use std::{
    net::SocketAddrV6,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::Result;
use rsnano_output_tracker::{OutputListenerMt, OutputTrackerMt};
use store_traits::{
    environment::StoreCursor,
    ledger::{PeerStore, StoreIterator},
    transaction::{LedgerReadTxn, LedgerWriteTxn},
    types::{StoreDatabase, StoreWriteFlags},
};

use crate::{PEERS_CF_NAME, RocksdbCursor, RocksdbStoreEnvironment, rocksdb_ro_cursor_from_store};

pub struct RocksdbPeerStore {
    database: StoreDatabase,
    put_listener: OutputListenerMt<(SocketAddrV6, SystemTime)>,
    delete_listener: OutputListenerMt<SocketAddrV6>,
}

impl RocksdbPeerStore {
    pub fn new(env: Arc<RocksdbStoreEnvironment>) -> Result<Self> {
        let database = env.open_db(Some(PEERS_CF_NAME))?;
        Ok(Self {
            database,
            put_listener: OutputListenerMt::new(),
            delete_listener: OutputListenerMt::new(),
        })
    }

    fn database(&self) -> StoreDatabase {
        self.database
    }

    pub fn track_puts(&self) -> Arc<OutputTrackerMt<(SocketAddrV6, SystemTime)>> {
        self.put_listener.track()
    }

    pub fn track_deletions(&self) -> Arc<OutputTrackerMt<SocketAddrV6>> {
        self.delete_listener.track()
    }

    pub fn put(&self, txn: &mut dyn LedgerWriteTxn, endpoint: SocketAddrV6, time: SystemTime) {
        if self.put_listener.is_tracked() {
            self.put_listener.emit((endpoint, time));
        }
        let key = encode_endpoint(&endpoint);
        let value = encode_time(time);
        txn.put(self.database(), &key, &value, StoreWriteFlags::default())
            .expect("failed to store peer");
    }

    pub fn del(&self, txn: &mut dyn LedgerWriteTxn, endpoint: SocketAddrV6) {
        if self.delete_listener.is_tracked() {
            self.delete_listener.emit(endpoint);
        }
        let key = encode_endpoint(&endpoint);
        txn.delete(self.database(), &key, None)
            .expect("failed to delete peer");
    }

    pub fn exists(&self, txn: &dyn LedgerReadTxn, endpoint: SocketAddrV6) -> bool {
        let key = encode_endpoint(&endpoint);
        txn.raw_exists(self.database(), &key)
    }

    pub fn iter<'txn>(
        &'txn self,
        txn: &'txn dyn LedgerReadTxn,
    ) -> StoreIterator<'txn, (SocketAddrV6, SystemTime)> {
        let cursor = txn
            .open_ro_cursor(self.database())
            .expect("failed to open peer cursor");
        let cursor = rocksdb_ro_cursor_from_store(cursor);
        Box::new(RocksdbPeerIterator::new(cursor))
    }

    pub fn count(&self, txn: &dyn LedgerReadTxn) -> u64 {
        txn.raw_count(self.database())
    }

    pub fn clear(&self, txn: &mut dyn LedgerWriteTxn) {
        txn.clear_db(self.database())
            .expect("failed to clear peers");
    }
}

fn encode_endpoint(endpoint: &SocketAddrV6) -> [u8; 18] {
    let mut bytes = [0u8; 18];
    bytes[..16].copy_from_slice(&endpoint.ip().octets());
    bytes[16..].copy_from_slice(&endpoint.port().to_be_bytes());
    bytes
}

fn decode_endpoint(bytes: &[u8]) -> SocketAddrV6 {
    let ip: [u8; 16] = bytes[..16]
        .try_into()
        .expect("invalid peer endpoint length");
    let port: [u8; 2] = bytes[16..18]
        .try_into()
        .expect("invalid peer endpoint length");
    SocketAddrV6::new(ip.into(), u16::from_be_bytes(port), 0, 0)
}

fn encode_time(time: SystemTime) -> [u8; 8] {
    let duration = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let millis = duration.as_millis();
    let clamped = if millis > u64::MAX as u128 {
        u64::MAX
    } else {
        millis as u64
    };
    clamped.to_be_bytes()
}

fn decode_time(bytes: &[u8]) -> SystemTime {
    let array: [u8; 8] = bytes[..8]
        .try_into()
        .expect("invalid peer timestamp length");
    UNIX_EPOCH + Duration::from_millis(u64::from_be_bytes(array))
}

struct RocksdbPeerIterator<'txn> {
    cursor: RocksdbCursor<'txn>,
}

impl<'txn> RocksdbPeerIterator<'txn> {
    fn new(cursor: RocksdbCursor<'txn>) -> Self {
        Self { cursor }
    }
}

impl<'txn> Iterator for RocksdbPeerIterator<'txn> {
    type Item = (SocketAddrV6, SystemTime);

    fn next(&mut self) -> Option<Self::Item> {
        let entry = self.cursor.next().expect("failed to advance cursor")?;
        let endpoint = decode_endpoint(entry.0.as_ref());
        let time = decode_time(entry.1.as_ref());
        Some((endpoint, time))
    }
}

impl PeerStore for RocksdbPeerStore {
    fn put(&self, txn: &mut dyn LedgerWriteTxn, endpoint: SocketAddrV6, time: SystemTime) {
        RocksdbPeerStore::put(self, txn, endpoint, time);
    }

    fn del(&self, txn: &mut dyn LedgerWriteTxn, endpoint: SocketAddrV6) {
        RocksdbPeerStore::del(self, txn, endpoint);
    }

    fn exists(&self, txn: &dyn LedgerReadTxn, endpoint: SocketAddrV6) -> bool {
        RocksdbPeerStore::exists(self, txn, endpoint)
    }

    fn iter<'a>(
        &'a self,
        txn: &'a dyn LedgerReadTxn,
    ) -> StoreIterator<'a, (SocketAddrV6, SystemTime)> {
        RocksdbPeerStore::iter(self, txn)
    }

    fn track_puts(&self) -> Arc<OutputTrackerMt<(SocketAddrV6, SystemTime)>> {
        RocksdbPeerStore::track_puts(self)
    }

    fn track_deletions(&self) -> Arc<OutputTrackerMt<SocketAddrV6>> {
        RocksdbPeerStore::track_deletions(self)
    }
}
