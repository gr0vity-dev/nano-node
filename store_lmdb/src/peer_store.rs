use std::{
    array::TryFromSliceError,
    net::SocketAddrV6,
    ops::Deref,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use rsnano_nullable_lmdb::{
    ConfiguredDatabase, DatabaseFlags, LmdbDatabase, LmdbEnvironment, WriteFlags,
};
use rsnano_output_tracker::{OutputListenerMt, OutputTrackerMt};
use store_traits::{
    transaction::{LedgerReadTxn, LedgerWriteTxn},
    types::StoreDatabase,
};

use crate::{
    PEERS_TEST_DATABASE,
    iterator::LmdbIterator,
    store_utils::{lmdb_ro_cursor_from_store, store_database_from_lmdb, store_write_flags_from},
};

pub struct LmdbPeerStore {
    database: LmdbDatabase,
    put_listener: OutputListenerMt<(SocketAddrV6, SystemTime)>,
    delete_listener: OutputListenerMt<SocketAddrV6>,
}

impl LmdbPeerStore {
    pub fn new(env: &LmdbEnvironment) -> anyhow::Result<Self> {
        let database = env.create_db(Some("peers"), DatabaseFlags::empty())?;

        Ok(Self {
            database,
            put_listener: OutputListenerMt::new(),
            delete_listener: OutputListenerMt::new(),
        })
    }

    pub fn database(&self) -> LmdbDatabase {
        self.database
    }

    fn store_database(&self) -> StoreDatabase {
        store_database_from_lmdb(self.database)
    }

    pub fn track_puts(&self) -> Arc<OutputTrackerMt<(SocketAddrV6, SystemTime)>> {
        self.put_listener.track()
    }

    pub fn put(&self, txn: &mut dyn LedgerWriteTxn, endpoint: SocketAddrV6, time: SystemTime) {
        self.put_listener.emit((endpoint.clone(), time));
        txn.put(
            self.store_database(),
            &EndpointBytes::from(endpoint),
            &TimeBytes::from(time),
            store_write_flags_from(WriteFlags::empty()),
        )
        .unwrap();
    }

    pub fn track_deletions(&self) -> Arc<OutputTrackerMt<SocketAddrV6>> {
        self.delete_listener.track()
    }

    pub fn del(&self, txn: &mut dyn LedgerWriteTxn, endpoint: SocketAddrV6) {
        self.delete_listener.emit(endpoint);
        txn.delete(self.store_database(), &EndpointBytes::from(endpoint), None)
            .unwrap();
    }

    pub fn exists(&self, txn: &dyn LedgerReadTxn, endpoint: SocketAddrV6) -> bool {
        match txn.get(self.store_database(), &EndpointBytes::from(endpoint)) {
            Ok(_) => true,
            Err(e) if e.is_not_found() => false,
            Err(e) => panic!("Could not check peer entry: {:?}", e),
        }
    }

    pub fn count(&self, txn: &dyn LedgerReadTxn) -> u64 {
        txn.count(self.store_database())
    }

    pub fn clear(&self, txn: &mut dyn LedgerWriteTxn) {
        txn.clear_db(self.store_database()).unwrap();
    }

    pub fn iter<'a>(
        &self,
        txn: &'a dyn LedgerReadTxn,
    ) -> impl Iterator<Item = (SocketAddrV6, SystemTime)> + 'a + use<'a> {
        let cursor = txn
            .open_ro_cursor(self.store_database())
            .expect("Could not read peer store database");
        let cursor = lmdb_ro_cursor_from_store(cursor);
        PeerIterator(LmdbIterator::new(cursor, |k, v| {
            (
                EndpointBytes::try_from(k).unwrap().into(),
                TimeBytes::try_from(v).unwrap().into(),
            )
        }))
    }
}

pub struct PeerIterator<'txn>(LmdbIterator<'txn, EndpointBytes, TimeBytes>);

impl<'txn> Iterator for PeerIterator<'txn> {
    type Item = (SocketAddrV6, SystemTime);

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(|(k, v)| (k.into(), v.into()))
    }
}

struct EndpointBytes([u8; 18]);

impl Deref for EndpointBytes {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl TryFrom<&[u8]> for EndpointBytes {
    type Error = TryFromSliceError;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        let buffer: [u8; 18] = value.try_into()?;
        Ok(Self(buffer))
    }
}

impl From<SocketAddrV6> for EndpointBytes {
    fn from(value: SocketAddrV6) -> Self {
        let mut bytes = [0; 18];
        let (ip, port) = bytes.split_at_mut(16);
        ip.copy_from_slice(&value.ip().octets());
        port.copy_from_slice(&value.port().to_be_bytes());
        Self(bytes)
    }
}

impl From<EndpointBytes> for SocketAddrV6 {
    fn from(value: EndpointBytes) -> Self {
        let (ip, port) = value.0.split_at(16);
        let ip: [u8; 16] = ip.try_into().unwrap();
        let port: [u8; 2] = port.try_into().unwrap();
        SocketAddrV6::new(ip.into(), u16::from_be_bytes(port), 0, 0)
    }
}

pub struct TimeBytes([u8; 8]);

impl Deref for TimeBytes {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl TryFrom<&[u8]> for TimeBytes {
    type Error = TryFromSliceError;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        let buffer: [u8; 8] = value.try_into()?;
        Ok(Self(buffer))
    }
}

impl From<SystemTime> for TimeBytes {
    fn from(value: SystemTime) -> Self {
        Self(
            (value
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64)
                .to_be_bytes(),
        )
    }
}

impl From<TimeBytes> for SystemTime {
    fn from(value: TimeBytes) -> Self {
        UNIX_EPOCH + Duration::from_millis(u64::from_be_bytes(value.0))
    }
}

pub struct ConfiguredPeersDatabaseBuilder {
    database: ConfiguredDatabase,
}

impl ConfiguredPeersDatabaseBuilder {
    pub fn new() -> Self {
        Self {
            database: ConfiguredDatabase::new(PEERS_TEST_DATABASE, "peers"),
        }
    }

    pub fn peer(mut self, endpoint: SocketAddrV6, time: SystemTime) -> Self {
        self.database.insert(
            EndpointBytes::from(endpoint).to_vec(),
            TimeBytes::from(time).to_vec(),
        );
        self
    }

    pub fn build(self) -> ConfiguredDatabase {
        self.database
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transaction::{LmdbLedgerReadTxn, LmdbLedgerWriteTxn};
    use rsnano_nullable_lmdb::{DeleteEvent, PutEvent};
    use std::{
        net::Ipv6Addr,
        time::{Duration, UNIX_EPOCH},
    };

    #[test]
    fn empty_store() {
        let fixture = Fixture::new();
        let txn = fixture.begin_read();
        let store = &fixture.store;
        assert_eq!(store.count(&txn), 0);
        assert_eq!(store.exists(&txn, TEST_PEER_A), false);
        assert_eq!(store.iter(&txn).next(), None);
    }

    #[test]
    fn add_one_endpoint() {
        let fixture = Fixture::new();
        let mut txn = fixture.begin_write();
        let put_tracker = txn.as_inner_mut().track_puts();

        let key = TEST_PEER_A;
        let time = UNIX_EPOCH + Duration::from_secs(1261440000);
        fixture.store.put(&mut txn, key, time);

        assert_eq!(
            put_tracker.output(),
            vec![PutEvent {
                database: LmdbDatabase::new_null(42),
                key: vec![0, 1, 0, 2, 0, 3, 0, 4, 0, 5, 0, 6, 0, 7, 0, 8, 0x3, 0xE8],
                value: 1261440000000u64.to_be_bytes().to_vec(),
                flags: WriteFlags::empty()
            }]
        )
    }

    #[test]
    fn exists() {
        let fixture = Fixture::with_stored_data(vec![TEST_PEER_A.clone(), TEST_PEER_B.clone()]);

        let txn = fixture.begin_read();

        assert_eq!(fixture.store.exists(&txn, TEST_PEER_A), true);
        assert_eq!(fixture.store.exists(&txn, TEST_PEER_B), true);
        assert_eq!(fixture.store.exists(&txn, UNKNOWN_PEER), false);
    }

    #[test]
    fn count() {
        let fixture = Fixture::with_stored_data(vec![TEST_PEER_A, TEST_PEER_B]);
        let txn = fixture.begin_read();
        assert_eq!(fixture.store.count(&txn), 2);
    }

    #[test]
    fn delete() {
        let fixture = Fixture::new();
        let mut txn = fixture.begin_write();
        let delete_tracker = txn.as_inner_mut().track_deletions();

        fixture.store.del(&mut txn, TEST_PEER_A);

        assert_eq!(
            delete_tracker.output(),
            vec![DeleteEvent {
                database: LmdbDatabase::new_null(42),
                key: EndpointBytes::from(TEST_PEER_A).to_vec()
            }]
        )
    }

    #[test]
    fn track_puts() {
        let fixture = Fixture::new();
        let mut txn = fixture.begin_write();
        let time = UNIX_EPOCH + Duration::from_secs(1261440000);
        let put_tracker = fixture.store.track_puts();

        fixture.store.put(&mut txn, TEST_PEER_A, time);

        let output = put_tracker.output();
        assert_eq!(output, vec![(TEST_PEER_A, time)]);
    }

    #[test]
    fn track_deletes() {
        let fixture = Fixture::new();
        let mut txn = fixture.begin_write();
        let delete_tracker = fixture.store.track_deletions();

        fixture.store.del(&mut txn, TEST_PEER_A);

        let output = delete_tracker.output();
        assert_eq!(output, vec![TEST_PEER_A]);
    }

    const TEST_PEER_A: SocketAddrV6 =
        SocketAddrV6::new(Ipv6Addr::new(1, 2, 3, 4, 5, 6, 7, 8), 1000, 0, 0);

    const TEST_PEER_B: SocketAddrV6 =
        SocketAddrV6::new(Ipv6Addr::new(3, 3, 3, 3, 3, 3, 3, 3), 2000, 0, 0);

    const UNKNOWN_PEER: SocketAddrV6 =
        SocketAddrV6::new(Ipv6Addr::new(4, 4, 4, 4, 4, 4, 4, 4), 4000, 0, 0);

    struct Fixture {
        env: Arc<LmdbEnvironment>,
        store: LmdbPeerStore,
    }

    impl Fixture {
        fn new() -> Self {
            Self::with_env(LmdbEnvironment::new_null())
        }

        fn with_stored_data(entries: Vec<SocketAddrV6>) -> Self {
            let mut env =
                LmdbEnvironment::null_builder().database("peers", LmdbDatabase::new_null(42));

            for entry in entries {
                env = env.entry(&EndpointBytes::from(entry), &[]);
            }

            Self::with_env(env.build().build())
        }

        fn with_env(env: LmdbEnvironment) -> Self {
            let env = Arc::new(env);
            Self {
                store: LmdbPeerStore::new(&env).unwrap(),
                env,
            }
        }

        fn begin_read(&self) -> LmdbLedgerReadTxn {
            LmdbLedgerReadTxn::new(self.env.begin_read())
        }

        fn begin_write(&self) -> LmdbLedgerWriteTxn {
            LmdbLedgerWriteTxn::new(self.env.begin_write())
        }
    }
}
