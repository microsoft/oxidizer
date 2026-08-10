// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Process-retained allocation domains.

use std::{fmt, ptr};

use crate::backend::{self, Backend, RawDomain};

/// An error produced while creating an allocation domain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum CreationError {
    /// No process allocator registered a domain backend.
    BackendUnavailable,
    /// The installed backend could not create an independent domain.
    CreationFailed,
    /// The installed backend returned an invalid null domain target.
    InvalidTarget,
}

impl fmt::Display for CreationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BackendUnavailable => formatter.write_str("no allocation domain backend is installed"),
            Self::CreationFailed => formatter.write_str("the allocation backend could not create a domain"),
            Self::InvalidTarget => formatter.write_str("the allocation backend returned a null domain target"),
        }
    }
}

impl std::error::Error for CreationError {}

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
    /// Unlike [`Domain::process`], each successful call asks the backend for a
    /// distinct domain.
    ///
    /// # Panics
    ///
    /// Panics when [`Domain::try_independent`] returns an error.
    #[must_use]
    pub fn new() -> Self {
        Self::try_independent().unwrap_or_else(|error| panic!("{error}"))
    }

    /// Returns the backend's shared process-default allocation domain.
    ///
    /// Repeated calls return the same backend-native domain.
    ///
    /// # Panics
    ///
    /// Panics if no allocation backend is installed or it returns an invalid
    /// null process-domain target.
    #[must_use]
    pub fn process() -> Self {
        Self::default()
    }

    /// Attempts to create an independent allocation domain.
    ///
    /// # Errors
    ///
    /// Returns a typed error when no backend is installed, domain creation
    /// fails, or the backend returns an invalid null target.
    pub fn try_independent() -> Result<Self, CreationError> {
        let backend = backend::installed().ok_or(CreationError::BackendUnavailable)?;
        let raw = (backend.create_domain)().ok_or(CreationError::CreationFailed)?;
        let raw = validate_independent_target(raw)?;
        Ok(Self { target: raw, backend })
    }

    /// Attempts to create an independent allocation domain.
    ///
    /// Returns `None` when no allocation backend is installed, when the backend
    /// cannot create a domain, or when it returns an invalid null target.
    #[must_use]
    pub fn try_new() -> Option<Self> {
        Self::try_independent().ok()
    }

    /// Wraps a backend-native domain target.
    ///
    /// # Safety
    ///
    /// `raw` must be a non-null domain target owned by `backend` and remain
    /// valid for every heap that carries this domain.
    ///
    /// # Panics
    ///
    /// Panics if `raw` is null.
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
    /// # Panics
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

fn validate_independent_target(raw: RawDomain) -> Result<RawDomain, CreationError> {
    if raw.target().is_null() {
        Err(CreationError::InvalidTarget)
    } else {
        Ok(raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn independent_targets_reject_null_backend_values() {
        let raw = unsafe { RawDomain::new(ptr::null_mut()) };
        assert_eq!(validate_independent_target(raw), Err(CreationError::InvalidTarget));
    }
}
