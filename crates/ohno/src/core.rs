// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::backtrace::BacktraceStatus;
use std::borrow::Cow;
use std::error::Error as StdError;
use std::fmt;

use super::EnrichmentEntry;
use super::backtrace::Backtrace;
use super::source::Source;

/// Internal error data that is boxed to keep `OhnoCore` lightweight.
#[derive(Debug, Clone)]
pub struct Inner {
    pub(super) source: Source,
    pub(super) backtrace: Backtrace,
    pub(super) enrichment: Vec<EnrichmentEntry>,
}

/// Core error type that wraps source errors, captures backtraces, and holds enrichment entries.
///
/// `OhnoCore` is the foundation of the ohno error handling system. It can wrap any error
/// type while providing automatic backtrace capture and enrichment message stacking capabilities.
///
/// The internal error data is boxed to keep the `Err` variant in `Result` small. This minimizes
/// cases where the `Err` is larger than the `Ok` variant. If the error only contains a
/// `OhnoCore` field, the size of `Err` will be equivalent to that of a raw pointer.
///
/// # Examples
///
/// ```rust
/// use ohno::OhnoCore;
///
/// // Create from a string message
/// let core = OhnoCore::builder().error("something went wrong").build();
///
/// // Wrap an existing error
/// let io_error = std::io::Error::new(std::io::ErrorKind::NotFound, "file.txt");
/// let wrapped = OhnoCore::builder().error(io_error).build();
/// ```
#[derive(Clone)]
pub struct OhnoCore {
    pub(super) data: Box<Inner>,
}

impl OhnoCore {
    pub(crate) fn from_builder(builder: crate::OhnoCoreBuilder) -> Self {
        Self {
            data: Box::new(Inner {
                source: builder.error,
                backtrace: match builder.backtrace_policy {
                    crate::BacktracePolicy::Auto => Backtrace::capture(),
                    crate::BacktracePolicy::Never => Backtrace::disabled(),
                    crate::BacktracePolicy::Forced => Backtrace::force_capture(),
                },
                enrichment: Vec::new(),
            }),
        }
    }

    /// Creates a new `OhnoCore` with no source. Automatically captures a backtrace.
    #[must_use]
    pub fn new() -> Self {
        Self::from_source(Source::None)
    }

    /// Creates a new [`OhnoCoreBuilder`](crate::OhnoCoreBuilder) for configuring an `OhnoCore` instance.
    #[must_use]
    pub fn builder() -> crate::OhnoCoreBuilder {
        crate::OhnoCoreBuilder::new()
    }

    fn from_source(source: Source) -> Self {
        Self {
            data: Box::new(Inner {
                source,
                backtrace: Backtrace::capture(),
                enrichment: Vec::new(),
            }),
        }
    }

    /// Returns the source error if this error wraps another error.
    #[must_use]
    pub fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match &self.data.source {
            Source::Error(source) => Some(source.as_ref()),
            Source::Transparent(source) => source.source(),
            Source::None => None,
        }
    }

    /// Returns whether this error has a captured backtrace.
    #[must_use]
    pub fn has_backtrace(&self) -> bool {
        matches!(self.data.backtrace.status(), BacktraceStatus::Captured)
    }

    /// Returns a reference to the backtrace regardless of capture status.
    ///
    /// This method always returns a reference to the internal backtrace,
    /// even if it wasn't captured (in which case it will be empty/disabled).
    pub fn backtrace(&self) -> &std::backtrace::Backtrace {
        self.data.backtrace.as_backtrace()
    }

    /// Returns an iterator over the enrichment information in reverse order (most recent first).
    pub fn enrichments(&self) -> impl Iterator<Item = &EnrichmentEntry> {
        self.data.enrichment.iter().rev()
    }

    /// Returns an iterator over just the enrichment messages in reverse order (most recent first).
    pub fn enrichment_messages(&self) -> impl Iterator<Item = &str> {
        self.data.enrichment.iter().rev().map(|ctx| ctx.message.as_ref())
    }

    /// Formats the main error message without backtrace and error enrichment.
    #[must_use]
    pub fn format_message(&self, default_message: &str, override_message: Option<Cow<'_, str>>) -> String {
        MessageFormatter {
            core: self,
            default_message,
            override_message,
        }
        .to_string()
    }

    /// Formats the error with an optional custom message override.
    ///
    /// This method is used internally by the Display implementation and by
    /// derived Error types that want to override the main error message.
    ///
    /// # Errors
    ///
    /// This function returns a `fmt::Error` if writing to the formatter fails.
    pub fn format_error(&self, f: &mut fmt::Formatter<'_>, default_message: &str, override_message: Option<Cow<'_, str>>) -> fmt::Result {
        let m = MessageFormatter {
            core: self,
            default_message,
            override_message,
        };

        std::fmt::Display::fmt(&m, f)?;

        for ctx in &self.data.enrichment {
            write!(f, "\n> {ctx}")?;
        }

        if matches!(self.data.backtrace.status(), BacktraceStatus::Captured) {
            write!(f, "\n\nBacktrace:\n{}", self.data.backtrace.as_backtrace())?;
        }

        Ok(())
    }
}

impl std::fmt::Debug for OhnoCore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OhnoCore")
            .field("source", &self.data.source)
            .field("backtrace", &self.data.backtrace)
            .field("enrichment", &self.data.enrichment)
            .finish()
    }
}

impl fmt::Display for OhnoCore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.format_error(f, "", None)
    }
}

impl Default for OhnoCore {
    fn default() -> Self {
        Self::new()
    }
}

/// Helper struct for formatting error messages in a consistent way.
struct MessageFormatter<'a> {
    core: &'a OhnoCore,
    default_message: &'a str,
    override_message: Option<Cow<'a, str>>,
}

impl fmt::Display for MessageFormatter<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const CAUSED_BY: &str = "caused by:";

        let MessageFormatter {
            core,
            default_message,
            override_message,
        } = self;

        match (override_message, &core.data.source) {
            (Some(msg), Source::Transparent(source) | Source::Error(source)) => {
                write!(f, "{msg}\n{CAUSED_BY} {source}")
            }
            (Some(msg), Source::None) => write!(f, "{msg}"),
            (None, Source::Transparent(source) | Source::Error(source)) => write!(f, "{source}"),
            (None, Source::None) => write!(f, "{default_message}"),
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::enrichable::Enrichable;

    #[test]
    fn test_default() {
        let error = OhnoCore::default();
        assert!(matches!(error.data.source, Source::None));
    }

    #[test]
    fn test_format_error() {
        let error = OhnoCore::builder().error("test error").build();
        let result = error.to_string();
        assert!(result.contains("test error"));
    }

    #[test]
    fn test_new() {
        let error = OhnoCore::new();
        assert!(matches!(error.data.source, Source::None));
        assert!(error.data.enrichment.is_empty());
    }

    #[test]
    fn test_from_string() {
        let error = OhnoCore::builder().error("msg").build();
        assert!(error.source().is_none());
        if let Source::Transparent(source) = &error.data.source {
            assert_eq!(source.to_string(), "msg");
        }
        assert!(matches!(&error.data.source, Source::Transparent(_)), "expected transparent source");
    }

    #[test]
    fn test_caused_by_without_backtrace() {
        let io_error = std::io::Error::other("io error");
        let error = OhnoCore::builder()
            .backtrace_policy(crate::BacktracePolicy::Never)
            .error(io_error)
            .build();
        assert!(matches!(error.data.source, Source::Error(_)));
        assert!(!error.has_backtrace());
        assert!(error.source().unwrap().downcast_ref::<std::io::Error>().is_some());
    }

    #[test]
    fn test_caused_by() {
        let io_error = std::io::Error::other("io error");
        let error = OhnoCore::builder().error(io_error).build();
        assert!(matches!(error.data.source, Source::Error(_)));
        assert!(error.source().unwrap().downcast_ref::<std::io::Error>().is_some());
    }

    #[test]
    fn test_from_boxed_error() {
        let io_error = std::io::Error::other("io error");
        let boxed: Box<dyn StdError + Send + Sync> = Box::new(io_error);
        let error = OhnoCore::builder().error(boxed).build();
        assert!(matches!(error.data.source, Source::Error(_)));
        assert!(error.source().unwrap().downcast_ref::<std::io::Error>().is_some());
    }

    #[test]
    fn test_from_boxed_error_2() {
        let io_error = std::io::Error::other("io error");
        let boxed: Box<dyn StdError + Send + Sync> = Box::new(io_error);
        let error = OhnoCore::builder().error(boxed).build();
        assert!(matches!(error.data.source, Source::Error(_)));
        assert!(error.source().unwrap().downcast_ref::<std::io::Error>().is_some());
    }

    #[test]
    fn test_enrichment_iter_and_messages() {
        let mut error = OhnoCore::builder().error("msg").build();
        error.add_enrichment(EnrichmentEntry::new("ctx1", "test.rs", 1));
        error.add_enrichment(EnrichmentEntry::new("ctx2", "test.rs", 2));
        let messages: Vec<_> = error.enrichment_messages().collect();
        assert_eq!(messages, vec!["ctx2", "ctx1"]);
    }

    #[test]
    fn test_display_and_debug() {
        let error = OhnoCore::builder().error("msg").build();
        let display = format!("{error}");
        assert!(display.starts_with("msg"));
        let debug = format!("{error:?}");
        assert!(debug.contains("OhnoCore"));
    }

    #[test]
    fn test_from_string_impls() {
        let error1 = OhnoCore::builder().error("abc").build();
        assert!(error1.to_string().starts_with("abc"));
        assert!(matches!(error1.data.source, Source::Transparent(_)));

        let error2 = OhnoCore::builder().error(String::from("def")).build();
        assert!(error2.to_string().starts_with("def"));
        assert!(matches!(error2.data.source, Source::Transparent(_)));

        let error3 = OhnoCore::builder().error(Cow::Borrowed("ghi")).build();
        assert!(error3.to_string().starts_with("ghi"));
        assert!(matches!(error3.data.source, Source::Transparent(_)));
    }

    #[test]
    fn test_from_boxed_error_impl() {
        let io_error = std::io::Error::other("io error");
        let boxed: Box<dyn StdError + Send + Sync> = Box::new(io_error);
        let error = OhnoCore::builder().error(boxed).build();
        assert!(matches!(error.data.source, Source::Error(_)));
        assert!(error.source().unwrap().downcast_ref::<std::io::Error>().is_some());
    }

    #[test]
    fn test_from_io_error_impl() {
        let io_error = std::io::Error::other("io error");
        let error = OhnoCore::builder().error(io_error).build();
        assert!(matches!(error.data.source, Source::Error(_)));
        assert!(error.source().unwrap().downcast_ref::<std::io::Error>().is_some());
    }

    #[test]
    #[cfg_attr(miri, ignore)] // unsupported operation: `GetCurrentDirectoryW` not available when isolation is enabled
    fn force_backtrace_capture() {
        let error = OhnoCore::builder()
            .backtrace_policy(crate::BacktracePolicy::Forced)
            .error("test error with backtrace")
            .build();

        assert!(error.has_backtrace());
        let backtrace = error.backtrace();
        assert_eq!(backtrace.status(), BacktraceStatus::Captured);
        let display = format!("{error}");
        assert!(display.starts_with("test error with backtrace\n\nBacktrace:\n"));
    }

    #[test]
    fn no_backtrace_capture() {
        let error = OhnoCore::builder()
            .backtrace_policy(crate::BacktracePolicy::Never)
            .error("test error without backtrace")
            .build();
        assert!(!error.has_backtrace());
        assert_eq!(error.backtrace().status(), BacktraceStatus::Disabled);
        let display = format!("{error}");
        assert_eq!(display, "test error without backtrace");
    }
}
