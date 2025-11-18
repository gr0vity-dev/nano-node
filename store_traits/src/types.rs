use std::{
    borrow::Borrow,
    fmt,
    marker::PhantomData,
    num::NonZeroUsize,
    ops::{BitAnd, BitAndAssign, BitOr, BitOrAssign, Deref},
    sync::Arc,
};

pub type StoreResult<T> = Result<T, StoreError>;

/// Owned value returned by store backends.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct StoreValue(Arc<[u8]>);

impl StoreValue {
    pub fn new(bytes: Arc<[u8]>) -> Self {
        Self(bytes)
    }

    pub fn from_slice(slice: &[u8]) -> Self {
        Self(Arc::<[u8]>::from(slice))
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn into_arc(self) -> Arc<[u8]> {
        self.0
    }
}

impl Deref for StoreValue {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

impl From<Vec<u8>> for StoreValue {
    fn from(value: Vec<u8>) -> Self {
        Self(Arc::from(value.into_boxed_slice()))
    }
}

impl From<Box<[u8]>> for StoreValue {
    fn from(value: Box<[u8]>) -> Self {
        Self(Arc::from(value))
    }
}

impl From<&[u8]> for StoreValue {
    fn from(value: &[u8]) -> Self {
        Self::from_slice(value)
    }
}

impl From<Arc<[u8]>> for StoreValue {
    fn from(value: Arc<[u8]>) -> Self {
        Self(value)
    }
}

impl From<StoreValue> for Arc<[u8]> {
    fn from(value: StoreValue) -> Self {
        value.into_arc()
    }
}

impl AsRef<[u8]> for StoreValue {
    fn as_ref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl Borrow<[u8]> for StoreValue {
    fn borrow(&self) -> &[u8] {
        self.as_slice()
    }
}

/// Backend database handles are encoded as opaque `NonZeroUsize` values.  Backends
/// must use `from_raw`/`into_raw` to wrap and unwrap their own representations,
/// keeping the meaning of the bits private to the backend crate.
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
