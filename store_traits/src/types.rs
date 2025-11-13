use std::{
    fmt,
    marker::PhantomData,
    num::NonZeroUsize,
    ops::{BitAnd, BitAndAssign, BitOr, BitOrAssign},
};

use rsnano_nullable_lmdb::{Error as LmdbError, LmdbDatabase};

pub type StoreResult<T> = Result<T, StoreError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoreDatabase(NonZeroUsize);

impl StoreDatabase {
    /// Creates a `StoreDatabase` from a raw non-zero handle.
    ///
    /// # Safety
    /// Callers must ensure the handle originates from a backend-specific database id
    /// that stays valid for as long as this value is used.
    pub const unsafe fn from_raw(raw: NonZeroUsize) -> Self {
        Self(raw)
    }

    /// Returns the raw encoded handle so backend adapters can reinterpret it.
    pub const fn into_raw(self) -> NonZeroUsize {
        self.0
    }

    /// Creates a handle from its encoded representation.
    pub fn from_usize(value: usize) -> Option<Self> {
        NonZeroUsize::new(value).map(Self)
    }
}

const HANDLE_STUB_FLAG: usize = 1;

impl From<LmdbDatabase> for StoreDatabase {
    fn from(value: LmdbDatabase) -> Self {
        let (is_stub, id) = value.to_raw_parts();
        let encoded = encode_handle(id, is_stub);
        unsafe { Self::from_raw(encoded) }
    }
}

impl From<StoreDatabase> for LmdbDatabase {
    fn from(value: StoreDatabase) -> Self {
        let (id, is_stub) = decode_handle(value.into_raw());
        if is_stub {
            LmdbDatabase::from_raw_parts(true, id)
        } else {
            LmdbDatabase::from_raw_parts(false, id)
        }
    }
}

fn encode_handle(id: u32, is_stub: bool) -> NonZeroUsize {
    let mut raw = ((id as usize) << 1) | if is_stub { HANDLE_STUB_FLAG } else { 0 };
    raw += 1;
    NonZeroUsize::new(raw).expect("raw handles are always incremented")
}

fn decode_handle(raw: NonZeroUsize) -> (u32, bool) {
    let mut value = raw.get() - 1;
    let is_stub = (value & HANDLE_STUB_FLAG) == HANDLE_STUB_FLAG;
    value >>= 1;
    (value as u32, is_stub)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreErrorKind {
    NotFound,
    MapFull,
    InvalidArgument,
    Corruption,
    PageNotFound,
    Backend,
}

#[derive(Debug)]
pub struct StoreError {
    kind: StoreErrorKind,
    message: Box<str>,
}

impl StoreError {
    pub fn new(kind: StoreErrorKind, message: impl Into<Box<str>>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn backend(message: impl Into<Box<str>>) -> Self {
        Self::new(StoreErrorKind::Backend, message)
    }

    pub fn not_found() -> Self {
        Self::new(StoreErrorKind::NotFound, "record not found")
    }

    pub fn kind(&self) -> StoreErrorKind {
        self.kind
    }

    pub fn is_not_found(&self) -> bool {
        matches!(self.kind, StoreErrorKind::NotFound)
    }
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for StoreError {}

impl From<LmdbError> for StoreError {
    fn from(value: LmdbError) -> Self {
        match value {
            LmdbError::NotFound => StoreError::not_found(),
            LmdbError::MapFull => {
                StoreError::new(StoreErrorKind::MapFull, value.to_string().into_boxed_str())
            }
            LmdbError::Invalid => StoreError::new(
                StoreErrorKind::InvalidArgument,
                value.to_string().into_boxed_str(),
            ),
            LmdbError::Corrupted => StoreError::new(
                StoreErrorKind::Corruption,
                value.to_string().into_boxed_str(),
            ),
            LmdbError::PageNotFound => StoreError::new(
                StoreErrorKind::PageNotFound,
                value.to_string().into_boxed_str(),
            ),
            _ => StoreError::backend(value.to_string()),
        }
    }
}

type CursorDropper = unsafe fn(NonZeroUsize);

#[derive(Debug)]
pub struct StoreRoCursor<'txn> {
    handle: NonZeroUsize,
    dropper: CursorDropper,
    _marker: PhantomData<&'txn ()>,
}

impl<'txn> StoreRoCursor<'txn> {
    /// # Safety
    /// The caller must ensure `handle` identifies a valid backend cursor and that `dropper`
    /// knows how to reclaim it exactly once.
    pub const unsafe fn from_raw_parts(handle: NonZeroUsize, dropper: CursorDropper) -> Self {
        Self {
            handle,
            dropper,
            _marker: PhantomData,
        }
    }

    pub const fn raw_handle(&self) -> NonZeroUsize {
        self.handle
    }

    pub fn into_raw_parts(self) -> (NonZeroUsize, CursorDropper) {
        let handle = self.handle;
        let dropper = self.dropper;
        std::mem::forget(self);
        (handle, dropper)
    }
}

impl Drop for StoreRoCursor<'_> {
    fn drop(&mut self) {
        unsafe { (self.dropper)(self.handle) };
    }
}

#[derive(Debug)]
pub struct StoreRwCursor<'txn> {
    handle: NonZeroUsize,
    dropper: CursorDropper,
    _marker: PhantomData<&'txn ()>,
}

impl<'txn> StoreRwCursor<'txn> {
    /// # Safety
    /// The caller must ensure `handle` identifies a valid backend cursor and that `dropper`
    /// knows how to reclaim it exactly once.
    pub const unsafe fn from_raw_parts(handle: NonZeroUsize, dropper: CursorDropper) -> Self {
        Self {
            handle,
            dropper,
            _marker: PhantomData,
        }
    }

    pub const fn raw_handle(&self) -> NonZeroUsize {
        self.handle
    }

    pub fn into_raw_parts(self) -> (NonZeroUsize, CursorDropper) {
        let handle = self.handle;
        let dropper = self.dropper;
        std::mem::forget(self);
        (handle, dropper)
    }
}

impl Drop for StoreRwCursor<'_> {
    fn drop(&mut self) {
        unsafe { (self.dropper)(self.handle) };
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoreWriteFlags(u32);

impl StoreWriteFlags {
    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    pub const fn bits(self) -> u32 {
        self.0
    }
}

impl Default for StoreWriteFlags {
    fn default() -> Self {
        Self::empty()
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
pub struct StoreEnvironmentFlags(u32);

impl StoreEnvironmentFlags {
    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    pub const fn bits(self) -> u32 {
        self.0
    }
}

impl Default for StoreEnvironmentFlags {
    fn default() -> Self {
        Self::empty()
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
