use crate::ledger::datastore::LedgerHandle;
use rsnano_core::BlockHash;
use rsnano_node::cementation::ConfirmingSet;
use std::{ops::Deref, sync::Arc, time::Duration};

pub struct ConfirmingSetHandle(pub Arc<ConfirmingSet>);

impl Deref for ConfirmingSetHandle {
    type Target = Arc<ConfirmingSet>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[no_mangle]
pub extern "C" fn rsn_confirming_set_create(
    ledger: &LedgerHandle,
    batch_time_ms: u64,
) -> *mut ConfirmingSetHandle {
    Box::into_raw(Box::new(ConfirmingSetHandle(Arc::new(ConfirmingSet::new(
        Arc::clone(ledger),
        Duration::from_millis(batch_time_ms),
    )))))
}

#[no_mangle]
pub unsafe extern "C" fn rsn_confirming_set_destroy(handle: *mut ConfirmingSetHandle) {
    drop(Box::from_raw(handle))
}

#[no_mangle]
pub unsafe extern "C" fn rsn_confirming_set_add(handle: &mut ConfirmingSetHandle, hash: *const u8) {
    handle.0.add(BlockHash::from_ptr(hash));
}

#[no_mangle]
pub extern "C" fn rsn_confirming_set_start(handle: &mut ConfirmingSetHandle) {
    handle.0.start();
}

#[no_mangle]
pub extern "C" fn rsn_confirming_set_stop(handle: &mut ConfirmingSetHandle) {
    handle.0.stop();
}

#[no_mangle]
pub unsafe extern "C" fn rsn_confirming_set_exists(
    handle: &mut ConfirmingSetHandle,
    hash: *const u8,
) -> bool {
    handle.0.exists(&BlockHash::from_ptr(hash))
}

#[no_mangle]
pub unsafe extern "C" fn rsn_confirming_set_len(handle: &mut ConfirmingSetHandle) -> usize {
    handle.0.len()
}
