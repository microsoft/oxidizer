//! Process-retained allocation domains.

use std::fmt;
use std::ptr;

use crate::backend::{self, Backend, RawDomain};

/// A process-retained, backend-native allocation domain.
#[derive(Clone, Copy)]
pub struct Domain {
    target: RawDomain,
    backend: &'static Backend,
}

impl PartialEq for Domain {
    fn eq(&self, other: &Self) -> bool {
        self.target == other.target && ptr::eq(self.backend, other.backend)
    }
}

impl Eq for Domain {}

impl Domain {
    /// Creates an independent allocation domain.
    ///
    /// # Panics
    ///
    /// Panics if no allocation backend is installed or the backend cannot
    /// create the domain.
    #[must_use]
    pub fn new() -> Self {
        let backend = backend::installed().unwrap_or_else(|| panic!("no allocation domain backend is installed"));
        let raw = (backend.create_domain)().unwrap_or_else(|| panic!("the allocation backend could not create a domain"));
        Self {
            target: valid_target(raw),
            backend,
        }
    }

    /// Attempts to create an independent allocation domain.
    ///
    /// Returns `None` when no allocation backend is installed, when the backend
    /// cannot create a domain, or when it returns an invalid null target.
    #[must_use]
    pub fn try_new() -> Option<Self> {
        let backend = backend::installed()?;
        let raw = (backend.create_domain)()?;
        (!raw.target().is_null()).then_some(Self { target: raw, backend })
    }

    /// Wraps a backend-native domain target.
    ///
    /// # Safety
    ///
    /// `raw` must be a non-null domain target owned by `backend` and remain
    /// valid for every heap that carries this domain.
    #[doc(hidden)]
    #[must_use]
    pub unsafe fn from_raw(raw: RawDomain, backend: &'static Backend) -> Self {
        Self {
            target: valid_target(raw),
            backend,
        }
    }

    /// Returns this domain's backend-native target.
    ///
    /// Panics if `backend` does not own this domain.
    #[doc(hidden)]
    #[must_use]
    pub fn raw_for(self, backend: &'static Backend) -> RawDomain {
        assert!(ptr::eq(self.backend, backend), "allocation domain belongs to a different backend");
        self.target
    }
}

impl Default for Domain {
    fn default() -> Self {
        let backend = backend::installed().unwrap_or_else(|| panic!("no allocation domain backend is installed"));
        let raw = (backend.default_domain)();
        Self {
            target: valid_target(raw),
            backend,
        }
    }
}

impl fmt::Debug for Domain {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Domain")
            .field("identity", &self.target.target())
            .finish_non_exhaustive()
    }
}

fn valid_target(raw: RawDomain) -> RawDomain {
    assert!(!raw.target().is_null(), "allocation backend returned a null domain target");
    raw
}
