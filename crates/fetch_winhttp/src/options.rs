// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Exposes the transport-specific configuration `fetch` cannot express.
//!
//! Generic `fetch` client options describe deadlines and protocol preferences
//! in terms every transport shares, but a few `WinHTTP` behaviors have no
//! generic counterpart. This module holds the caller-facing values for those
//! behaviors - the native DNS-resolution deadline in [`WinHttpOptions`]
//! (design.md section 6.1) - together with [`ProtocolOptions`], the validated
//! result of mapping a caller's requested HTTP versions onto the native
//! protocol flags (design.md section 3, implementation.md section 10.1).
//!
//! Nothing here touches the operating system. Encoding these values into
//! native representations is [`crate::convert`]'s responsibility, and applying
//! them to a handle is [`crate::request`]'s and [`crate::session`]'s, which
//! keeps the public surface free of FFI detail.

use std::time::Duration;

use thread_aware::ThreadAware;

/// Configures native behavior specific to the WinHTTP transport.
///
/// The resolve timeout is a native DNS-only deadline because generic `fetch`
/// options have no separately awaitable name-resolution stage. Generic connect,
/// response-header, body-idle, and pipeline request timeouts remain responsible
/// for their broader intervals and are not replaced by this option.
///
/// By default the native DNS resolution timeout is unlimited.
#[derive(Clone, Debug, Default, ThreadAware)]
#[non_exhaustive]
pub struct WinHttpOptions {
    resolve_timeout: Option<Duration>,
}

impl WinHttpOptions {
    /// Starts building WinHTTP-specific transport options.
    #[must_use]
    pub fn builder() -> WinHttpOptionsBuilder {
        WinHttpOptionsBuilder { options: Self::default() }
    }

    pub(crate) fn resolve_timeout(&self) -> Option<Duration> {
        self.resolve_timeout
    }
}

/// Builds [`WinHttpOptions`] without changing generic request deadlines.
///
/// It configures only the native DNS-resolution timer; all other timeout
/// responsibilities remain with the generic client and request options.
#[derive(Clone, Debug, ThreadAware)]
#[non_exhaustive]
pub struct WinHttpOptionsBuilder {
    options: WinHttpOptions,
}

impl WinHttpOptionsBuilder {
    /// Sets the native DNS resolution timeout.
    ///
    /// This timeout covers DNS resolution only. Other request deadlines are
    /// configured through the generic `fetch` client options.
    ///
    /// Configured values below one millisecond are rounded up to one
    /// millisecond. Values beyond the signed [`WinHttpSetTimeouts`] parameter
    /// range are clamped.
    ///
    /// [`WinHttpSetTimeouts`]: https://learn.microsoft.com/windows/win32/api/winhttp/nf-winhttp-winhttpsettimeouts
    #[must_use]
    pub fn resolve_timeout(mut self, timeout: Duration) -> Self {
        self.options.resolve_timeout = Some(timeout);
        self
    }

    /// Builds the WinHTTP-specific transport options.
    #[must_use]
    pub fn build(self) -> WinHttpOptions {
        self.options
    }
}

/// Translates a supported HTTP version set into WinHTTP request options.
///
/// The mask enables HTTP/2 and HTTP/3 because HTTP/1.1 is the WinHTTP baseline.
/// `required` is set whenever HTTP/1.1 is absent, and must be applied with the
/// mask so WinHTTP fails negotiation instead of silently downgrading. Unsupported
/// versions are rejected before this value is constructed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ProtocolOptions {
    advanced_mask: u32,
    required: bool,
}

impl ProtocolOptions {
    /// Creates the option pair from an already validated version set.
    ///
    /// This is a plain field initializer; it does not itself check the pairing.
    /// The invariant - `required` is set exactly when the requested versions
    /// omit HTTP/1.1 - is established by the sole construction site,
    /// [`crate::convert::protocol_options`], which is where the version set is
    /// validated and where the rest of the native value encoding lives.
    pub(crate) const fn new(advanced_mask: u32, required: bool) -> Self {
        Self { advanced_mask, required }
    }

    pub(crate) const fn advanced_mask(self) -> u32 {
        self.advanced_mask
    }

    pub(crate) const fn required(self) -> bool {
        self.required
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::fmt::Debug;
    use std::panic::{RefUnwindSafe, UnwindSafe};
    use std::time::Duration;

    use static_assertions::assert_impl_all;
    use thread_aware::ThreadAware;

    use super::{ProtocolOptions, WinHttpOptions, WinHttpOptionsBuilder};

    assert_impl_all!(WinHttpOptions: Send, Sync, Clone, Debug, Default, ThreadAware, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(WinHttpOptionsBuilder: Send, Sync, Clone, Debug, ThreadAware, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(ProtocolOptions: Send, Sync, Clone, Copy, Debug, Eq, PartialEq, UnwindSafe, RefUnwindSafe);

    #[test]
    fn default_leaves_resolve_timeout_unlimited() {
        assert_eq!(WinHttpOptions::default().resolve_timeout(), None);
    }

    #[test]
    fn builder_sets_resolve_timeout() {
        let timeout = Duration::from_secs(10);
        let options = WinHttpOptions::builder().resolve_timeout(timeout).build();

        assert_eq!(options.resolve_timeout(), Some(timeout));
    }

    #[test]
    fn protocol_options_expose_the_constructed_pair() {
        let options = ProtocolOptions::new(3, true);

        assert_eq!(options.advanced_mask(), 3);
        assert!(options.required());
    }
}
