use lmdb_sys::MDB_dbi;
use std::mem;

#[derive(Clone, Debug, PartialEq, Eq, Copy)]
pub struct LmdbDatabase(DatabaseType);

impl LmdbDatabase {
    pub const fn new(db: lmdb::Database) -> Self {
        Self(DatabaseType::Real(db))
    }

    pub const fn new_null(id: u32) -> Self {
        Self(DatabaseType::Stub(id))
    }

    pub fn as_real(&self) -> lmdb::Database {
        let DatabaseType::Real(db) = &self.0 else {
            panic!("database handle was not a real handle");
        };
        *db
    }

    pub fn as_nulled(&self) -> u32 {
        let DatabaseType::Stub(db) = self.0 else {
            panic!("database handle was not a nulled handle");
        };
        db
    }

    pub fn to_raw_parts(&self) -> (bool, u32) {
        match self.0 {
            DatabaseType::Real(db) => (false, db.dbi() as u32),
            DatabaseType::Stub(id) => (true, id),
        }
    }

    pub fn from_raw_parts(is_stub: bool, id: u32) -> Self {
        if is_stub {
            Self(DatabaseType::Stub(id))
        } else {
            Self(DatabaseType::Real(unsafe { database_from_dbi(id) }))
        }
    }
}

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
enum DatabaseType {
    Real(lmdb::Database),
    Stub(u32),
}

unsafe fn database_from_dbi(dbi: u32) -> lmdb::Database {
    // `lmdb::Database` is a thin wrapper around `MDB_dbi`.
    unsafe { mem::transmute::<MDB_dbi, lmdb::Database>(dbi as MDB_dbi) }
}
