// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Canonical error types for the flush surface.

use std::borrow::Cow;
use std::error::Error as StdError;
use std::fmt;

use ohno::{ErrorExt, OhnoCore};

/// One processor's failure to flush.
///
/// Returned by [`EventProcessor::flush`](crate::processing::EventProcessor::flush).
/// It names the processor that failed and keeps the underlying error as the
/// cause, so a caller can both attribute the failure and reach the original
/// error through [`ErrorExt::find_source`].
///
/// # Examples
///
/// ```
/// use observed::FlushError;
///
/// let io = std::io::Error::other("disk full");
/// let err = FlushError::new("otel-logs", io);
///
/// assert_eq!(err.processor(), "otel-logs");
/// ```
#[derive(ohno::Error)]
#[no_constructors]
#[display("processor `{processor}` failed to flush")]
pub struct FlushError {
    processor: Cow<'static, str>,
    ohno_core: OhnoCore,
}

impl FlushError {
    /// Reports that `processor` failed to flush, keeping `source` as the cause.
    ///
    /// A `Sink` only ever sees `dyn EventProcessor`, so the identity has to be
    /// supplied here, at the implementation site. Prefer a stable, curated name
    /// over [`type_name`](std::any::type_name): the name is read by whoever
    /// triages the failure, and a type path both leaks private module structure
    /// and changes when the type is moved.
    #[must_use]
    pub fn new(processor: impl Into<Cow<'static, str>>, source: impl Into<Box<dyn StdError + Send + Sync>>) -> Self {
        Self {
            processor: processor.into(),
            ohno_core: OhnoCore::from(source.into()),
        }
    }

    /// The processor that failed.
    #[must_use]
    pub fn processor(&self) -> &str {
        &self.processor
    }
}

/// Every processor failure from one flush pass.
///
/// Returned by [`Sink::flush`](crate::Sink::flush). A sink flushes all of its
/// processors even after one fails, so this reports the complete set rather
/// than only the first failure.
///
/// A processor that owns a processor list of its own builds one of these with
/// [`from_failures`](Self::from_failures) and reports it as the cause of its
/// own [`FlushError`].
///
/// # Examples
///
/// ```
/// use observed::{FlushError, SinkFlushError};
///
/// let err = SinkFlushError::from_failures(vec![
///     FlushError::new("otel-logs", std::io::Error::other("disk full")),
///     FlushError::new("otel-metrics", "endpoint unreachable"),
/// ]);
///
/// let names: Vec<_> = err.failures().iter().map(FlushError::processor).collect();
/// assert_eq!(names, ["otel-logs", "otel-metrics"]);
/// ```
#[ohno::error]
pub struct SinkFlushError;

impl SinkFlushError {
    /// Collects the failures from one flush pass into a single error.
    ///
    /// `failures` must be **every** failure the pass produced, in the order the
    /// processors were flushed. That is a contract on the caller, not something
    /// this constructor can check: the vector is stored as given, and both
    /// [`failures`](Self::failures) and the error message repeat it verbatim. A
    /// partial or reordered list therefore misreports the pass.
    ///
    /// # Panics
    ///
    /// Panics when `failures` is empty: a pass that produced no failure has
    /// nothing to report and belongs in the `Ok` arm, so an empty list is a
    /// caller mistake rather than a representable error.
    #[must_use]
    #[track_caller]
    pub fn from_failures(failures: Vec<FlushError>) -> Self {
        assert!(
            !failures.is_empty(),
            "SinkFlushError needs at least one failure; a clean flush is Ok(())"
        );

        Self::caused_by(Failures { failures })
    }

    /// The individual failures, in the order the processors were flushed.
    ///
    /// The order is the one the constructor was given; see
    /// [`from_failures`](Self::from_failures).
    #[must_use]
    pub fn failures(&self) -> &[FlushError] {
        self.0
            .source()
            .and_then(<dyn StdError + 'static>::downcast_ref::<Failures>)
            .map_or(&[], |f| &f.failures)
    }
}

/// The failure list underlying a [`SinkFlushError`].
///
/// Private because it exists only to be that error's source: keeping the
/// [`FlushError`]s live in the error chain - reachable through
/// [`SinkFlushError::failures`] and, for the first one, through the standard
/// [`StdError::source`] chain - instead of flattening them into a message
/// string at construction time.
#[derive(Debug)]
struct Failures {
    failures: Vec<FlushError>,
}

impl fmt::Display for Failures {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let count = self.failures.len();
        let plural = if count == 1 { "" } else { "s" };
        write!(f, "{count} processor{plural} failed to flush:")?;

        for failure in &self.failures {
            write!(f, "\n- {}", failure.message())?;
        }

        Ok(())
    }
}

impl StdError for Failures {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.failures.first().map(|first| first as &(dyn StdError + 'static))
    }
}

#[cfg(test)]
mod tests {
    use ohno::assert_error_message;

    use super::*;

    #[test]
    fn is_send_and_sync() {
        static_assertions::assert_impl_all!(FlushError: Send, Sync);
        static_assertions::assert_impl_all!(SinkFlushError: Send, Sync);
    }

    #[test]
    fn flush_error_names_the_processor_and_keeps_the_cause() {
        let err = FlushError::new("alpha", std::io::Error::other("disk full"));

        assert_eq!(err.processor(), "alpha");
        assert_error_message!(err, "processor `alpha` failed to flush");
        assert_eq!(
            err.find_source::<std::io::Error>().map(std::string::ToString::to_string),
            Some("disk full".to_owned())
        );
    }

    /// An empty list is a caller mistake, not a representable error: the
    /// `assert!` in `from_failures` is what keeps a "0 processors failed"
    /// value from ever being built.
    #[test]
    #[should_panic(expected = "SinkFlushError needs at least one failure")]
    fn no_failures_is_rejected() {
        let _ = SinkFlushError::from_failures(vec![]);
    }

    #[test]
    fn every_failure_is_reported() {
        let err = SinkFlushError::from_failures(vec![FlushError::new("alpha", "first"), FlushError::new("beta", "second")]);

        assert_eq!(err.failures().len(), 2);
        assert_eq!(err.failures()[0].processor(), "alpha");
        assert_eq!(err.failures()[1].processor(), "beta");

        assert_error_message!(
            err,
            "2 processors failed to flush:\n\
             - processor `alpha` failed to flush\ncaused by: first\n\
             - processor `beta` failed to flush\ncaused by: second"
        );
    }

    #[test]
    fn single_failure_message_is_singular() {
        let err = SinkFlushError::from_failures(vec![FlushError::new("alpha", "only")]);

        assert_error_message!(
            err,
            "1 processor failed to flush:\n\
             - processor `alpha` failed to flush\ncaused by: only"
        );
    }

    /// The first failure stays reachable through the standard source chain, so
    /// `find_source` still reaches the original error a processor reported.
    #[test]
    fn the_first_failure_is_in_the_source_chain() {
        let err = SinkFlushError::from_failures(vec![
            FlushError::new("alpha", std::io::Error::other("disk full")),
            FlushError::new("beta", "second"),
        ]);

        assert_eq!(
            err.find_source::<std::io::Error>().map(std::string::ToString::to_string),
            Some("disk full".to_owned())
        );
    }
}
