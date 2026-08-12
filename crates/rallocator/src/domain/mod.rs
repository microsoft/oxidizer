// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Region ownership domains shared by one or more heaps.

use std::ptr::NonNull;
use std::sync::OnceLock;

use allocation_hints::backend::RawDomain;
use allocation_hints::domain::Domain;

use crate::allocator::DomainState;

static DEFAULT_DOMAIN: OnceLock<DomainStatePtr> = OnceLock::new();
#[cfg(test)]
static FAIL_NEXT_DOMAIN_CREATION: std::sync::Mutex<Option<std::thread::ThreadId>> = std::sync::Mutex::new(None);

struct DomainStatePtr(NonNull<DomainState>);

// SAFETY: Domain states are process-retained, and DomainState synchronizes all
// mutable shared state before the pointer is dereferenced across threads.
unsafe impl Send for DomainStatePtr {}
unsafe impl Sync for DomainStatePtr {}

pub(crate) fn state(domain: Domain) -> *mut DomainState {
    domain.raw_for(crate::heap::backend()).target().cast()
}

pub(crate) unsafe fn from_state(state: *mut DomainState) -> Domain {
    unsafe { Domain::from_raw(RawDomain::new(state.cast()), crate::heap::backend()) }
}

pub(crate) fn default_state() -> *mut DomainState {
    default_domain().target().cast()
}

pub(crate) fn create_domain() -> Option<RawDomain> {
    if fail_next_domain_creation() {
        return None;
    }
    NonNull::new(crate::allocator::create_domain()).map(|state| unsafe { RawDomain::new(state.as_ptr().cast()) })
}

#[cfg(not(test))]
fn fail_next_domain_creation() -> bool {
    false
}

#[cfg(test)]
fn fail_next_domain_creation() -> bool {
    FAIL_NEXT_DOMAIN_CREATION
        .lock()
        .unwrap()
        .take_if(|owner| *owner == std::thread::current().id())
        .is_some()
}

pub(crate) fn default_domain() -> RawDomain {
    let state = DEFAULT_DOMAIN.get_or_init(|| {
        let domain = domain_or_else(create_domain(), || -> RawDomain { std::process::abort() });
        let state = domain.target().cast::<DomainState>();
        crate::allocator::mark_default_domain(state);
        DomainStatePtr(NonNull::new(state).expect("domain creation returned a validated non-null target"))
    });
    unsafe { RawDomain::new(state.0.as_ptr().cast()) }
}

fn domain_or_else(domain: Option<RawDomain>, on_failure: impl FnOnce() -> RawDomain) -> RawDomain {
    match domain {
        Some(domain) => domain,
        None => on_failure(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_creation_reports_mapping_failure() {
        crate::initialize();
        *FAIL_NEXT_DOMAIN_CREATION.lock().unwrap() = Some(std::thread::current().id());
        assert!(Domain::try_new().is_none());

        *FAIL_NEXT_DOMAIN_CREATION.lock().unwrap() = Some(std::thread::current().id());
        std::panic::catch_unwind(Domain::new).unwrap_err();
    }

    #[test]
    fn default_domain_delegates_unrecoverable_creation_failure() {
        std::panic::catch_unwind(|| domain_or_else(None, || panic!("injected domain creation failure"))).unwrap_err();
    }
}
