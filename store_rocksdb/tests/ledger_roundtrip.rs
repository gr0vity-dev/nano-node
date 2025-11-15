//! Integration-style litmus test for the RocksDB backend.
//!
//! The test will eventually spin a real ledger, create a small block lattice,
//! and assert that balance / pending iteration works through the store traits.

#[test]
#[ignore = "RocksDB ledger wiring not implemented yet"]
fn rocksdb_ledger_roundtrip() {
    // Skeleton placeholder: once RocksdbLedgerStoreFactory exists, the test
    // will instantiate it, insert a few blocks, and read them back.
    unimplemented!("RocksDB roundtrip coverage pending adapter implementation");
}
