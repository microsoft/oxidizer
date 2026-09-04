// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Process-retained allocation domains.

#[cfg(test)]
use std::fmt;
use std::ptr::NonNull;
use std::sync::OnceLock;

use crate::allocator::DomainState;

static DEFAULT_DOMAIN: OnceLock<DomainStatePtr> = OnceLock::new();
#[cfg(test)]
static FAIL_NEXT_DOMAIN_CREATION: std::sync::Mutex<Option<std::thread::ThreadId>> = std::sync::Mutex::new(None);

struct DomainStatePtr(NonNull<DomainState>);

// SAFETY: Domain states are process-retained, and DomainState synchronizes all
// mutable shared state before the pointer is dereferenced across threads.
unsafe impl Send for DomainStatePtr {}
// SAFETY: Sharing the process-retained pointer does not bypass DomainState synchronization.
unsafe impl Sync for DomainStatePtr {}

/// A process-retained allocation domain.
#[cfg(test)]
#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) struct Domain {
    state: NonNull<DomainState>,
}

// SAFETY: DomainState is process-retained and internally synchronizes shared state.
#[cfg(test)]
unsafe impl Send for Domain {}
// SAFETY: Sharing a Domain only copies its stable identity pointer.
#[cfg(test)]
unsafe impl Sync for Domain {}

#[cfg(test)]
impl Domain {
    /// Creates an independent allocation domain.
    ///
    /// Returns `None` when rallocator cannot reserve the domain's initial state.
    #[must_use]
    pub(crate) fn new() -> Option<Self> {
        crate::allocator::ensure_global_allocator_active().then_some(())?;
        NonNull::new(create_domain()).map(|state| Self { state })
    }

    /// Returns the shared process-default allocation domain.
    #[must_use]
    pub(crate) fn process() -> Self {
        Self {
            // SAFETY: default_state is initialized from a validated NonNull value.
            state: unsafe { NonNull::new_unchecked(default_state()) },
        }
    }
}

#[cfg(test)]
impl Default for Domain {
    fn default() -> Self {
        Self::process()
    }
}

#[cfg(test)]
impl fmt::Debug for Domain {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Domain")
            .field("identity", &self.state.as_ptr())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
pub(crate) const fn state(domain: Domain) -> *mut DomainState {
    domain.state.as_ptr()
}

pub(crate) fn default_state() -> *mut DomainState {
    DEFAULT_DOMAIN
        .get_or_init(|| {
            let state = domain_or_else(NonNull::new(create_domain()), || -> NonNull<DomainState> { std::process::abort() });
            crate::allocator::mark_default_domain(state.as_ptr());
            DomainStatePtr(state)
        })
        .0
        .as_ptr()
}

fn create_domain() -> *mut DomainState {
    if fail_next_domain_creation() {
        return std::ptr::null_mut();
    }
    crate::allocator::create_domain()
}

#[cfg(not(test))]
const fn fail_next_domain_creation() -> bool {
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

fn domain_or_else(domain: Option<NonNull<DomainState>>, on_failure: impl FnOnce() -> NonNull<DomainState>) -> NonNull<DomainState> {
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
        *FAIL_NEXT_DOMAIN_CREATION.lock().unwrap() = Some(std::thread::current().id());
        assert_eq!(Domain::new(), None);
    }

    #[test]
    fn default_domain_delegates_unrecoverable_creation_failure() {
        std::panic::catch_unwind(|| domain_or_else(None, || panic!("injected domain creation failure"))).unwrap_err();
    }

    #[test]
    fn domain_debug_reports_its_stable_identity() {
        let domain = Domain::new().unwrap();
        let identity = state(domain);

        let debug = format!("{domain:?}");

        assert!(debug.starts_with("Domain { identity: "));
        assert!(debug.contains(&format!("{identity:p}")));
        assert!(debug.ends_with(", .. }"));
    }
}
