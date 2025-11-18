use std::{
    borrow::Borrow,
    fmt,
    hash::{Hash, Hasher},
    marker::PhantomData,
    num::NonZeroUsize,
    ops::{BitAnd, BitAndAssign, BitOr, BitOrAssign, Deref},
    sync::Arc,
};

pub type StoreResult<T> = Result<T, StoreError>;

/// Owned or zero-copy buffer returned by store backends.
#[derive(Clone)]
pub struct StoreValue(StoreValueInner);

#[derive(Clone)]
enum StoreValueInner {
    Owned(Arc<[u8]>),
    Borrowed(Arc<dyn StoreValueBuffer>),
}

/// Backend-provided buffer that can expose its bytes without copying.
pub trait StoreValueBuffer: Send + Sync {
    fn as_slice(&self) -> &[u8];
}

impl StoreValue {
    pub fn new(bytes: Arc<[u8]>) -> Self {
        Self(StoreValueInner::Owned(bytes))
    }

    pub fn from_slice(slice: &[u8]) -> Self {
        Self::from(Arc::<[u8]>::from(slice))
    }

    pub fn from_borrowed<B>(buffer: B) -> Self
    where
        B: StoreValueBuffer + 'static,
    {
        Self(StoreValueInner::Borrowed(Arc::new(buffer)))
    }

    pub fn as_slice(&self) -> &[u8] {
        match &self.0 {
            StoreValueInner::Owned(bytes) => bytes.as_ref(),
            StoreValueInner::Borrowed(buffer) => buffer.as_slice(),
        }
    }

    pub fn len(&self) -> usize {
        self.as_slice().len()
    }

    pub fn is_empty(&self) -> bool {
        self.as_slice().is_empty()
    }

    pub fn into_arc(self) -> Arc<[u8]> {
        match self.0 {
            StoreValueInner::Owned(bytes) => bytes,
            StoreValueInner::Borrowed(buffer) => Arc::from(buffer.as_slice()),
        }
    }
}

impl fmt::Debug for StoreValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("StoreValue").field(&self.as_slice()).finish()
    }
}

impl PartialEq for StoreValue {
    fn eq(&self, other: &Self) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl Eq for StoreValue {}

impl Hash for StoreValue {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_slice().hash(state);
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
        Self::from(value.into_boxed_slice())
    }
}

impl From<Box<[u8]>> for StoreValue {
    fn from(value: Box<[u8]>) -> Self {
        Self(StoreValueInner::Owned(Arc::from(value)))
    }
}

impl From<&[u8]> for StoreValue {
    fn from(value: &[u8]) -> Self {
        Self::from_slice(value)
    }
}

impl From<Arc<[u8]>> for StoreValue {
    fn from(value: Arc<[u8]>) -> Self {
        Self(StoreValueInner::Owned(value))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct TestBuffer(Arc<[u8]>);

    impl StoreValueBuffer for TestBuffer {
        fn as_slice(&self) -> &[u8] {
            &self.0
        }
    }

    #[test]
    fn borrowed_value_exposes_bytes() {
        let data = Arc::<[u8]>::from(*b"borrowed");
        let value = StoreValue::from_borrowed(TestBuffer(Arc::clone(&data)));
        assert_eq!(value.as_slice(), b"borrowed");
        // into_arc copies borrowed data
        let owned: Arc<[u8]> = value.clone().into_arc();
        assert_eq!(&*owned, b"borrowed");
    }

    #[test]
    fn owned_value_roundtrips() {
        let value = StoreValue::from_slice(b"owned");
        assert_eq!(value.len(), 5);
        let arc: Arc<[u8]> = value.clone().into_arc();
        assert_eq!(&*arc, b"owned");
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
