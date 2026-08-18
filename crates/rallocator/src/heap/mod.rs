// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Configurable general-purpose and bump allocation heaps.

pub(crate) mod bump;

use std::alloc::Layout;
use std::ptr::{self, NonNull};

use allocation_hints::backend::{Backend, ClaimPolicy, RawHeap, RawHint};
use allocation_hints::domain::Domain;
#[cfg(test)]
use allocation_hints::heap::Heap;
use allocation_hints::heap::{
    CreationError, CreationPolicy, Info, InfoKind, Kind, Options, Usage, UsageKind, bump as common_bump, general as common_general,
};
use bump::BumpState;

use crate::allocator::{RemoteHeapState, ReusableHeapState};
use crate::hal;

#[cfg(test)]
static FAIL_NEXT_HEAP_MAPPING: std::sync::Mutex<Option<std::thread::ThreadId>> = std::sync::Mutex::new(None);
#[cfg(test)]
static FAIL_NEXT_BUMP_STATE: std::sync::Mutex<Option<std::thread::ThreadId>> = std::sync::Mutex::new(None);
#[cfg(test)]
static FAIL_NEXT_FALLBACK_ENSURE: std::sync::Mutex<Option<std::thread::ThreadId>> = std::sync::Mutex::new(None);

#[derive(Clone, Copy)]
pub(crate) enum HeapTarget {
    General(GeneralHeapPtr),
    Bump(BumpHeapPtr),
    Thread(RemoteHeapPtr),
}

#[derive(Clone, Copy)]
pub(crate) struct GeneralHeapPtr(NonNull<ReusableHeapState>);

#[derive(Clone, Copy)]
pub(crate) struct BumpHeapPtr(NonNull<BumpState>);

#[derive(Clone, Copy)]
pub(crate) struct RemoteHeapPtr(NonNull<RemoteHeapState>);

enum SupportedKind {
    General(common_general::Options),
    Bump(common_bump::Options),
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum SupportedCreationPolicy {
    Fresh,
    ThreadPool,
}

macro_rules! native_heap_pointer {
    ($name:ident, $target:ty) => {
        impl $name {
            fn new(pointer: *mut $target) -> Self {
                Self(NonNull::new(pointer).expect("allocator heap targets must not be null"))
            }

            pub(crate) const fn as_ptr(self) -> *mut $target {
                self.0.as_ptr()
            }
        }
    };
}

native_heap_pointer!(GeneralHeapPtr, ReusableHeapState);
native_heap_pointer!(BumpHeapPtr, BumpState);
native_heap_pointer!(RemoteHeapPtr, RemoteHeapState);

const TARGET_GENERAL: usize = 1;
const TARGET_BUMP: usize = 2;
const TARGET_THREAD: usize = 3;

static RALLOCATOR_BACKEND: Backend = unsafe {
    Backend::new(
        crate::domain::create_domain,
        crate::domain::default_domain,
        create_heap,
        create_thread_heap,
        crate::allocator::hint_thread_context,
        crate::allocator::set_active_hint,
        heap_info,
        heap_usage,
        destroy_hint,
    )
};

pub(crate) const fn backend() -> &'static Backend {
    &RALLOCATOR_BACKEND
}

pub(crate) fn ensure_backend_registered() {
    unsafe { allocation_hints::backend::register(backend()) };
}

fn create_thread_heap() -> Option<RawHeap> {
    let target = HeapTarget::Thread(RemoteHeapPtr::new(crate::allocator::thread_heap_state()?));
    Some(unsafe { RawHeap::new(raw_hint(target), ClaimPolicy::Shared) })
}

fn create_heap(options: Options) -> Result<RawHeap, CreationError> {
    let domain = options.domain().map_or_else(crate::domain::default_state, crate::domain::state);
    let creation_policy = supported_creation_policy(options.creation_policy())?;
    let target = match supported_kind(options.kind())? {
        SupportedKind::General(options) => {
            if creation_policy != SupportedCreationPolicy::Fresh {
                return Err(CreationError::CreationFailed);
            }
            HeapTarget::General(GeneralHeapPtr::new(
                new_general(options, domain).ok_or(CreationError::CreationFailed)?,
            ))
        }
        SupportedKind::Bump(options) => {
            let state = match creation_policy {
                SupportedCreationPolicy::Fresh => create_bump_state(options, domain),
                SupportedCreationPolicy::ThreadPool => {
                    crate::allocator::take_pooled_bump(domain).or_else(|| create_bump_state(options, domain))
                }
            }
            .ok_or(CreationError::CreationFailed)?;
            if creation_policy == SupportedCreationPolicy::ThreadPool && !ensure_bump_fallback_heap(state) {
                crate::allocator::return_pooled_bump(state);
                return Err(CreationError::CreationFailed);
            }
            unsafe { bump::reset_state(state, options) };
            HeapTarget::Bump(BumpHeapPtr::new(state))
        }
    };
    Ok(unsafe { RawHeap::new(raw_hint(target), ClaimPolicy::Exclusive) })
}

#[cfg_attr(coverage_nightly, coverage(off))]
fn supported_kind(kind: Kind) -> Result<SupportedKind, CreationError> {
    match kind {
        Kind::General(options) => Ok(SupportedKind::General(options)),
        Kind::Bump(options) => Ok(SupportedKind::Bump(options)),
        _ => Err(CreationError::CreationFailed),
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
fn supported_creation_policy(policy: CreationPolicy) -> Result<SupportedCreationPolicy, CreationError> {
    match policy {
        CreationPolicy::Fresh => Ok(SupportedCreationPolicy::Fresh),
        CreationPolicy::ThreadPool => Ok(SupportedCreationPolicy::ThreadPool),
        _ => Err(CreationError::CreationFailed),
    }
}

fn new_general(options: common_general::Options, domain: *mut crate::allocator::DomainState) -> Option<*mut ReusableHeapState> {
    let layout = Layout::new::<ReusableHeapState>();
    let state = map_heap_state(layout);
    if state.is_null() {
        return None;
    }
    unsafe { state.write(ReusableHeapState::new(options, domain)) };
    unsafe { crate::allocator::initialize_general_heap(state) };
    Some(state)
}

fn create_bump_state(options: common_bump::Options, domain: *mut crate::allocator::DomainState) -> Option<*mut BumpState> {
    if fail_next_bump_state() {
        return None;
    }
    bump::create_state(options, domain)
}

fn ensure_bump_fallback_heap(state: *mut BumpState) -> bool {
    if fail_next_fallback_ensure() {
        return false;
    }
    bump::ensure_fallback_heap(state)
}

fn map_heap_state(layout: Layout) -> *mut ReusableHeapState {
    if fail_next_heap_mapping() {
        return ptr::null_mut();
    }
    hal::map(layout.size()).cast()
}

#[cfg(not(test))]
fn fail_next_heap_mapping() -> bool {
    false
}

#[cfg(test)]
fn fail_next_heap_mapping() -> bool {
    take_failure_for_current_thread(&FAIL_NEXT_HEAP_MAPPING)
}

#[cfg(not(test))]
fn fail_next_bump_state() -> bool {
    false
}

#[cfg(test)]
fn fail_next_bump_state() -> bool {
    take_failure_for_current_thread(&FAIL_NEXT_BUMP_STATE)
}

#[cfg(not(test))]
fn fail_next_fallback_ensure() -> bool {
    false
}

#[cfg(test)]
fn fail_next_fallback_ensure() -> bool {
    take_failure_for_current_thread(&FAIL_NEXT_FALLBACK_ENSURE)
}

#[cfg(test)]
fn take_failure_for_current_thread(failure: &std::sync::Mutex<Option<std::thread::ThreadId>>) -> bool {
    failure
        .lock()
        .unwrap()
        .take_if(|owner| *owner == std::thread::current().id())
        .is_some()
}

unsafe fn domain_from_state(state: *mut crate::allocator::DomainState) -> Domain {
    unsafe { crate::domain::from_state(state) }
}

unsafe fn heap_info(hint: RawHint, claimed_active: bool) -> Info {
    let target = required_target(hint);
    let active = match target {
        HeapTarget::Thread(state) => crate::allocator::thread_heap_is_active(state.as_ptr()),
        _ => claimed_active,
    };
    let kind = match target {
        HeapTarget::General(state) => InfoKind::General(common_general::Info::new(
            unsafe { crate::allocator::general_heap_options(state.as_ptr()) },
            false,
        )),
        HeapTarget::Bump(state) => InfoKind::Bump(common_bump::Info::new(bump::options(unsafe { state.0.as_ref() }))),
        HeapTarget::Thread(state) => InfoKind::General(common_general::Info::new(
            unsafe { crate::allocator::thread_heap_options(state.as_ptr()) },
            true,
        )),
    };
    let domain = match target {
        HeapTarget::General(state) => unsafe { domain_from_state((*state.as_ptr()).domain) },
        HeapTarget::Bump(state) => unsafe { domain_from_state((*state.as_ptr()).domain) },
        HeapTarget::Thread(state) => unsafe { domain_from_state((*state.as_ptr()).domain) },
    };
    Info::new(active, domain, kind)
}

unsafe fn heap_usage(hint: RawHint) -> Result<Usage, ()> {
    match required_target(hint) {
        HeapTarget::General(state) => Ok(unsafe { crate::allocator::general_heap_usage(state.as_ptr(), ptr::null_mut()) }),
        HeapTarget::Bump(state) => {
            let (reserved_bytes, cursor_used_bytes, allocation_count, live_allocations, live_requested_bytes, chunk_count) =
                bump::usage(unsafe { state.0.as_ref() });
            Ok(Usage::new(
                live_allocations,
                live_requested_bytes,
                live_requested_bytes,
                reserved_bytes,
                reserved_bytes,
                UsageKind::Bump(common_bump::Usage::new(cursor_used_bytes, allocation_count, chunk_count)),
            ))
        }
        HeapTarget::Thread(state) => unsafe { crate::allocator::thread_heap_usage(state.as_ptr()) },
    }
}

unsafe fn destroy_hint(hint: RawHint) {
    match required_target(hint) {
        HeapTarget::General(state) => unsafe { crate::allocator::retire_general_heap(state.as_ptr()) },
        HeapTarget::Bump(state) => unsafe { bump::release_handle(state.as_ptr()) },
        HeapTarget::Thread(_) => {}
    }
}

fn raw_hint(target: HeapTarget) -> RawHint {
    match target {
        HeapTarget::General(state) => unsafe { RawHint::new(state.as_ptr().cast(), TARGET_GENERAL) },
        HeapTarget::Bump(state) => unsafe { RawHint::new(state.as_ptr().cast(), TARGET_BUMP) },
        HeapTarget::Thread(state) => unsafe { RawHint::new(state.as_ptr().cast(), TARGET_THREAD) },
    }
}

pub(crate) fn target_from_hint(hint: RawHint) -> Option<HeapTarget> {
    target_from_hint_or_else(hint, || -> Option<HeapTarget> { std::process::abort() })
}

fn target_from_hint_or_else(hint: RawHint, on_invalid: impl FnOnce() -> Option<HeapTarget>) -> Option<HeapTarget> {
    match hint.kind() {
        0 if hint.is_global() => None,
        TARGET_GENERAL => Some(HeapTarget::General(GeneralHeapPtr::new(hint.target().cast()))),
        TARGET_BUMP => Some(HeapTarget::Bump(BumpHeapPtr::new(hint.target().cast()))),
        TARGET_THREAD => Some(HeapTarget::Thread(RemoteHeapPtr::new(hint.target().cast()))),
        _ => on_invalid(),
    }
}

fn required_target(hint: RawHint) -> HeapTarget {
    required_target_or_else(hint, || -> HeapTarget { std::process::abort() })
}

fn required_target_or_else(hint: RawHint, on_invalid: impl FnOnce() -> HeapTarget) -> HeapTarget {
    match target_from_hint(hint) {
        Some(target) => target,
        None => on_invalid(),
    }
}

#[cfg(test)]
fn inject_failure(failure: &std::sync::Mutex<Option<std::thread::ThreadId>>) {
    *failure.lock().unwrap() = Some(std::thread::current().id());
}

#[cfg(test)]
fn exercise_heap_allocation_failures() {
    crate::initialize();
    inject_failure(&FAIL_NEXT_BUMP_STATE);
    assert_eq!(
        Heap::try_with_options(Options::bump(common_bump::Options::new())).unwrap_err(),
        CreationError::CreationFailed
    );

    inject_failure(&FAIL_NEXT_BUMP_STATE);
    std::panic::catch_unwind(|| Heap::from_thread_pool_in(Domain::new(), common_bump::Options::new())).unwrap_err();

    inject_failure(&FAIL_NEXT_FALLBACK_ENSURE);
    assert_eq!(
        Heap::try_with_options(Options::bump(common_bump::Options::new()).with_thread_pool()).unwrap_err(),
        CreationError::CreationFailed
    );
    let recovered = crate::allocator::take_pooled_bump(crate::domain::default_state())
        .expect("failed fallback initialization must return the pooled bump state");
    crate::allocator::return_pooled_bump(recovered);

    inject_failure(&FAIL_NEXT_HEAP_MAPPING);
    assert_eq!(Heap::try_new().unwrap_err(), CreationError::CreationFailed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heap_creation_reports_backing_allocation_failures() {
        crate::initialize();
        exercise_heap_allocation_failures();
    }

    #[test]
    fn heap_creation_rejects_general_pooling_and_invalid_hints_delegate_failure() {
        crate::initialize();
        assert_eq!(
            create_heap(Options::general(common_general::Options::new()).with_thread_pool()).unwrap_err(),
            CreationError::CreationFailed
        );

        assert!(std::panic::catch_unwind(|| required_target_or_else(RawHint::GLOBAL, || panic!("injected invalid global hint"))).is_err());
        let invalid = unsafe { RawHint::new(NonNull::<u8>::dangling().as_ptr().cast(), usize::MAX) };
        assert!(std::panic::catch_unwind(|| target_from_hint_or_else(invalid, || panic!("injected invalid target kind"))).is_err());
    }
}
