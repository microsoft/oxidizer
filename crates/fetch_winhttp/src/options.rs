// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Holds the validated native protocol selection.
//!
//! [`ProtocolOptions`] is the validated result of mapping a caller's requested
//! HTTP versions onto the native protocol flags (design.md section 3,
//! implementation.md section 10.1).
//!
//! Nothing here touches the operating system. Encoding these values into
//! native representations is [`crate::convert`]'s responsibility, and applying
//! them to a handle is [`crate::request`]'s and [`crate::session`]'s, which
//! keeps the public surface free of FFI detail.

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

    use static_assertions::assert_impl_all;

    use super::ProtocolOptions;

    assert_impl_all!(ProtocolOptions: Send, Sync, Clone, Copy, Debug, Eq, PartialEq, UnwindSafe, RefUnwindSafe);

    #[test]
    fn protocol_options_expose_the_constructed_pair() {
        let options = ProtocolOptions::new(3, true);

        assert_eq!(options.advanced_mask(), 3);
        assert!(options.required());
    }
}
