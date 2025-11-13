use std::{
    fmt,
    ops::{BitAnd, BitAndAssign, BitOr, BitOrAssign},
};

use rsnano_nullable_lmdb::{
    EnvironmentFlags, Error as LmdbError, LmdbDatabase, RoCursor as LmdbRoCursor,
    RwCursor as LmdbRwCursor, WriteFlags,
};

pub type StoreResult<T> = Result<T, StoreError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoreDatabase(LmdbDatabase);

impl StoreDatabase {
    pub fn new(inner: LmdbDatabase) -> Self {
        Self(inner)
    }

    pub fn inner(&self) -> &LmdbDatabase {
        &self.0
    }

    pub fn into_inner(self) -> LmdbDatabase {
        self.0
    }
}

impl From<LmdbDatabase> for StoreDatabase {
    fn from(value: LmdbDatabase) -> Self {
        Self(value)
    }
}

impl From<StoreDatabase> for LmdbDatabase {
    fn from(value: StoreDatabase) -> Self {
        value.0
    }
}

#[derive(Debug)]
pub struct StoreError(LmdbError);

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for StoreError {}

impl From<LmdbError> for StoreError {
    fn from(value: LmdbError) -> Self {
        Self(value)
    }
}

impl StoreError {
    pub fn is_not_found(&self) -> bool {
        matches!(self.0, LmdbError::NotFound)
    }

    pub fn as_lmdb_error(&self) -> &LmdbError {
        &self.0
    }
}

pub struct StoreRoCursor<'txn>(LmdbRoCursor<'txn>);

impl<'txn> StoreRoCursor<'txn> {
    pub fn new(inner: LmdbRoCursor<'txn>) -> Self {
        Self(inner)
    }

    pub fn inner(&self) -> &LmdbRoCursor<'txn> {
        &self.0
    }

    pub fn into_inner(self) -> LmdbRoCursor<'txn> {
        self.0
    }
}

pub struct StoreRwCursor<'txn>(LmdbRwCursor<'txn>);

impl<'txn> StoreRwCursor<'txn> {
    pub fn new(inner: LmdbRwCursor<'txn>) -> Self {
        Self(inner)
    }

    pub fn inner(&self) -> &LmdbRwCursor<'txn> {
        &self.0
    }

    pub fn inner_mut(&mut self) -> &mut LmdbRwCursor<'txn> {
        &mut self.0
    }

    pub fn into_inner(self) -> LmdbRwCursor<'txn> {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoreWriteFlags(WriteFlags);

impl StoreWriteFlags {
    pub fn empty() -> Self {
        Self(WriteFlags::empty())
    }

    pub fn bits(&self) -> WriteFlags {
        self.0
    }
}

impl Default for StoreWriteFlags {
    fn default() -> Self {
        Self::empty()
    }
}

impl From<WriteFlags> for StoreWriteFlags {
    fn from(value: WriteFlags) -> Self {
        Self(value)
    }
}

impl From<StoreWriteFlags> for WriteFlags {
    fn from(value: StoreWriteFlags) -> Self {
        value.0
    }
}

impl BitOr for StoreWriteFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for StoreWriteFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl BitAnd for StoreWriteFlags {
    type Output = Self;

    fn bitand(self, rhs: Self) -> Self::Output {
        Self(self.0 & rhs.0)
    }
}

impl BitAndAssign for StoreWriteFlags {
    fn bitand_assign(&mut self, rhs: Self) {
        self.0 &= rhs.0;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoreEnvironmentFlags(EnvironmentFlags);

impl StoreEnvironmentFlags {
    pub fn bits(&self) -> EnvironmentFlags {
        self.0
    }
}

impl Default for StoreEnvironmentFlags {
    fn default() -> Self {
        Self(EnvironmentFlags::empty())
    }
}

impl From<EnvironmentFlags> for StoreEnvironmentFlags {
    fn from(value: EnvironmentFlags) -> Self {
        Self(value)
    }
}

impl From<StoreEnvironmentFlags> for EnvironmentFlags {
    fn from(value: StoreEnvironmentFlags) -> Self {
        value.0
    }
}

impl BitOr for StoreEnvironmentFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for StoreEnvironmentFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}
