use crate::utils::ContainerInfoComponentHandle;
use rsnano_core::Amount;
use rsnano_node::vote_cache::VoteCache;
use std::ffi::{c_char, CStr};

pub struct VoteCacheHandle(VoteCache);

#[no_mangle]
pub extern "C" fn rsn_vote_cache_create(max_size: usize) -> *mut VoteCacheHandle {
    let tmp = Box::new(|_: &_| Amount::zero());
    Box::into_raw(Box::new(VoteCacheHandle(VoteCache::new(max_size, tmp))))
}

#[no_mangle]
pub unsafe extern "C" fn rsn_vote_cache_collect_container_info(
    handle: *const VoteCacheHandle,
    name: *const c_char,
) -> *mut ContainerInfoComponentHandle {
    let container_info = (*handle)
        .0
        .collect_container_info(CStr::from_ptr(name).to_str().unwrap().to_owned());
    Box::into_raw(Box::new(ContainerInfoComponentHandle(container_info)))
}
