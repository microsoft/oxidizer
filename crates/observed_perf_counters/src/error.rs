// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use observed::processing::{MetricBindingError, MetricReadError};
use ohno::ErrorExt;

/// A configuration, native publishing, or counter-processing failure.
///
/// Errors support enrichment and retain typed causes and original Windows status.
#[ohno::error]
#[derive(Clone)]
pub struct Error;

impl Error {
    pub(crate) fn configuration(reason: impl Into<InvalidConfiguration>) -> Self {
        reason.into().into()
    }

    /// Whether the application should select a different telemetry backend.
    ///
    /// This reports an unsupported operating system or pointer width, not a
    /// failure that retrying native startup on this process can fix.
    #[must_use]
    pub fn is_unsupported_platform(&self) -> bool {
        self.find_source::<UnsupportedPlatform>().is_some()
    }

    /// Whether startup requires correcting configuration rather than retrying.
    ///
    /// Includes invalid category/instance definitions and typed event bindings.
    #[must_use]
    pub fn is_invalid_configuration(&self) -> bool {
        self.find_source::<InvalidConfiguration>().is_some() || self.find_source::<MetricBindingError>().is_some()
    }

    /// The native status to include in operating-system failure diagnostics.
    ///
    /// Startup failures and frozen collection both retain their original status;
    /// this value does not imply that restarting or retrying is safe.
    #[must_use]
    pub fn windows_code(&self) -> Option<u32> {
        self.find_source::<NativeFailure>()
            .map(|cause| cause.code)
            .or_else(|| self.find_source::<CollectionFailure>().map(|cause| cause.code))
    }
}

impl From<MetricBindingError> for Error {
    fn from(source: MetricBindingError) -> Self {
        Self::caused_by(source)
    }
}

impl From<MetricReadError> for Error {
    fn from(source: MetricReadError) -> Self {
        Self::caused_by(source)
    }
}

// Each conversion preserves the leaf's stable message and its structured source.
macro_rules! cause_conversions {
    ($($cause:ty),+ $(,)?) => {
        $(
            impl From<$cause> for Error {
                fn from(source: $cause) -> Self {
                    Self::caused_by(source)
                }
            }
        )+
    };
}

cause_conversions!(
    InvalidConfiguration,
    UnsupportedPlatform,
    NativeFailure,
    Overflow,
    Underflow,
    CollectionFailure,
    Poisoned
);

// Typed causes stay private so callers need not exhaustively match backend internals.
#[ohno::error]
#[from(
    configuration::EmptyField,
    configuration::TextTooLong,
    configuration::InvalidXml,
    configuration::ControlCharacter,
    configuration::CounterPathCharacter,
    configuration::CounterCount,
    configuration::CounterIndexOverflow,
    configuration::UnsupportedAggregation,
    configuration::ApplicationIdentity,
    configuration::DuplicateName,
    configuration::IdCollision,
    configuration::UnsupportedMetric,
    configuration::MeasurementMismatch,
    configuration::PublishedCapacity,
    configuration::NativeTemplateSize,
    configuration::NativeCounterCount
)]
pub(crate) struct InvalidConfiguration;

// Distinct validation causes keep their inputs available without parsing messages.
pub(crate) mod configuration {
    use observed::metadata::{InstrumentKind, MetricValueKind};

    #[ohno::error]
    #[display("invalid performance-counter configuration: {field} must not be empty")]
    pub(crate) struct EmptyField {
        pub(crate) field: &'static str,
    }

    #[ohno::error]
    #[display("invalid performance-counter configuration: {field} exceeds {maximum} UTF-16 code units")]
    pub(crate) struct TextTooLong {
        pub(crate) field: &'static str,
        pub(crate) maximum: usize,
    }

    #[ohno::error]
    #[display("invalid performance-counter configuration: {field} contains NUL or an invalid XML character")]
    pub(crate) struct InvalidXml {
        pub(crate) field: &'static str,
    }

    #[ohno::error]
    #[display("invalid performance-counter configuration: {field} contains a control character")]
    pub(crate) struct ControlCharacter {
        pub(crate) field: &'static str,
    }

    #[ohno::error]
    #[display("invalid performance-counter configuration: {field} contains a Windows counter-path metacharacter")]
    pub(crate) struct CounterPathCharacter {
        pub(crate) field: &'static str,
    }

    #[ohno::error]
    #[display("invalid performance-counter configuration: bind between 1 and {maximum} counters (received {count})")]
    pub(crate) struct CounterCount {
        pub(crate) count: usize,
        pub(crate) maximum: usize,
    }

    #[ohno::error]
    #[display("invalid performance-counter configuration: too many counters")]
    pub(crate) struct CounterIndexOverflow;

    #[ohno::error]
    #[display("invalid performance-counter configuration: unsupported native metric aggregation {kind:?}")]
    pub(crate) struct UnsupportedAggregation {
        pub(crate) kind: InstrumentKind,
    }

    #[ohno::error]
    #[display("invalid performance-counter configuration: application identity must be a .dll or .exe filename without a directory")]
    pub(crate) struct ApplicationIdentity;

    #[ohno::error]
    #[display("invalid performance-counter configuration: duplicate counter display name {name:?}")]
    pub(crate) struct DuplicateName {
        pub(crate) name: String,
    }

    #[ohno::error]
    #[display("invalid performance-counter configuration: counter ID collision for {name:?}")]
    pub(crate) struct IdCollision {
        pub(crate) name: String,
    }

    #[ohno::error]
    #[display(
        "invalid performance-counter configuration: metric `{name}` requires an unsigned counter or gauge, or a signed up/down counter (received {kind:?}/{value_kind:?})"
    )]
    pub(crate) struct UnsupportedMetric {
        pub(crate) name: String,
        pub(crate) kind: InstrumentKind,
        pub(crate) value_kind: Option<MetricValueKind>,
    }

    #[ohno::error]
    #[display(
        "invalid performance-counter configuration: measurement does not match the prepared integer metric (expected {kind:?}, received {value_kind:?})"
    )]
    pub(crate) struct MeasurementMismatch {
        pub(crate) kind: InstrumentKind,
        pub(crate) value_kind: MetricValueKind,
    }

    #[ohno::error]
    #[display(
        "invalid performance-counter configuration: too many native counters active in this executable image ({count} exceeds {maximum})"
    )]
    pub(crate) struct PublishedCapacity {
        pub(crate) count: usize,
        pub(crate) maximum: usize,
    }

    #[ohno::error]
    #[display("invalid performance-counter configuration: native template exceeds ULONG")]
    pub(crate) struct NativeTemplateSize;

    #[ohno::error]
    #[display("invalid performance-counter configuration: native counter count exceeds ULONG")]
    pub(crate) struct NativeCounterCount;
}

#[ohno::error]
#[display("native performance counters require 64-bit Windows")]
pub(crate) struct UnsupportedPlatform;

#[ohno::error]
#[display("{operation} failed with Windows status {code}")]
pub(crate) struct NativeFailure {
    pub(crate) operation: &'static str,
    pub(crate) code: u32,
}

#[ohno::error]
#[display("metric {counter_id} overflowed u64; the update was rejected")]
pub(crate) struct Overflow {
    pub(crate) counter_id: u32,
}

#[ohno::error]
#[display("metric {counter_id} would fall below zero; the update was rejected")]
pub(crate) struct Underflow {
    pub(crate) counter_id: u32,
}

#[ohno::error]
#[display("performance-counter collection failed with Windows status {code}; snapshots are frozen")]
pub(crate) struct CollectionFailure {
    pub(crate) code: u32,
}

#[ohno::error]
#[display("performance-counter state lock was poisoned")]
pub(crate) struct Poisoned;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use ohno::EnrichableExt as _;

    use super::*;

    // Source errors and enrichment may retain arbitrary interior mutable state.
    static_assertions::assert_not_impl_any!(Error: UnwindSafe, RefUnwindSafe);

    #[test]
    fn predicates_distinguish_configuration_platform_and_native_failures() {
        let cases = [
            (
                Error::configuration(configuration::EmptyField::new("category")),
                (true, false, None),
            ),
            (UnsupportedPlatform::new().into(), (false, true, None)),
            (NativeFailure::new("start", 42_u32).into(), (false, false, Some(42))),
            (CollectionFailure::new(87_u32).into(), (false, false, Some(87))),
            (Overflow::new(7_u32).into(), (false, false, None)),
        ];
        for (error, expected) in cases {
            assert_eq!(
                (
                    error.is_invalid_configuration(),
                    error.is_unsupported_platform(),
                    error.windows_code()
                ),
                expected,
            );
            assert!(std::error::Error::source(&error).is_some());
        }
    }

    #[test]
    fn typed_causes_and_enrichment_survive_cloning() {
        let error = Error::from(NativeFailure::new("start", 42_u32)).enrich("starting worker counters");
        let cloned = error.clone();
        let cause = cloned.find_source::<NativeFailure>().unwrap();
        assert!(std::ptr::eq(error.find_source::<NativeFailure>().unwrap(), cause));
        assert_eq!((cause.operation, cause.code), ("start", 42));
        assert_eq!(cloned.message().lines().next().unwrap(), "start failed with Windows status 42");
        assert!(cloned.to_string().contains("starting worker counters"));
        let overflow = Error::from(Overflow::new(7_u32));
        assert_eq!(overflow.find_source::<Overflow>().unwrap().counter_id, 7);
        let underflow = Error::from(Underflow::new(9_u32));
        assert_eq!(underflow.find_source::<Underflow>().unwrap().counter_id, 9);
        let configuration = Error::configuration(configuration::EmptyField::new("category"));
        assert_eq!(configuration.find_source::<configuration::EmptyField>().unwrap().field, "category");
    }
}
