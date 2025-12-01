// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::backtrace::BacktraceStatus;
use std::borrow::Cow;
use std::error::Error as StdError;
use std::fmt;

use super::backtrace::Backtrace;
use super::source::Source;
use super::EnrichmentEntry;

/// Internal error data that is boxed to keep `OhnoCore` lightweight.
#[derive(Debug, Clone)]
pub struct Inner {
    pub(super) source: Source,
    pub(super) backtrace: Backtrace,
    pub(super) enrichment: Vec<EnrichmentEntry>,
}

/// Core error type that wraps source errors, captures backtraces, and holds enrichment traces.
///
/// `OhnoCore` is the foundation of the ohno error handling system. It can wrap any error
/// type while providing automatic backtrace capture and enrichment trace stacking capabilities.
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
/// let core = OhnoCore::from("something went wrong");
///
/// // Wrap an existing error
/// let io_error = std::io::Error::new(std::io::ErrorKind::NotFound, "file.txt");
/// let wrapped = OhnoCore::from(io_error);
/// ```
#[derive(Clone)]
pub struct OhnoCore {
    pub(super) data: Box<Inner>,
}

impl OhnoCore {
    /// Creates a new `OhnoCore` with no source (useful when using display override).
    ///
    /// Automatically captures a backtrace at the point of creation.
    ///
    /// # Examples
    ///
    /// ```rust
    /// let error = ohno::OhnoCore::new();
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self::from_source(Source::None)
    }

    /// Creates a new `OhnoCore` wrapping an existing error.
    ///
    /// The wrapped error becomes the source in the error chain. Backtrace capture
    /// is disabled assuming the source error already has one.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use std::io;
    ///
    /// use ohno::OhnoCore;
    ///
    /// let io_error = io::Error::new(io::ErrorKind::PermissionDenied, "access denied");
    /// let wrapped = OhnoCore::without_backtrace(io_error);
    /// ```
    pub fn without_backtrace(error: impl Into<Box<dyn StdError + Send + Sync + 'static>>) -> Self {
        Self {
            data: Box::new(Inner {
                source: Source::Error(error.into().into()),
                backtrace: Backtrace::disabled(),
                enrichment: Vec::new(),
            }),
        }
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
    ///
    /// # Examples
    ///
    /// ```rust
    /// use std::io;
    ///
    /// use ohno::OhnoCore;
    ///
    /// let io_error = io::Error::new(io::ErrorKind::NotFound, "file.txt");
    /// let wrapped = OhnoCore::from(io_error);
    ///
    /// assert!(wrapped.source().is_some());
    /// ```
    #[must_use]
    pub fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match &self.data.source {
            Source::Error(source) => Some(source.as_ref()),
            Source::Transparent(source) => source.source(),
            Source::None => None,
        }
    }

    /// Returns whether this error has a captured backtrace.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ohno::OhnoCore;
    ///
    /// let error = OhnoCore::from("test error");
    /// // Backtrace capture depends on RUST_BACKTRACE environment variable
    /// println!("Has backtrace: {}", error.has_backtrace());
    /// ```
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

    /// Formats the main error message without backtrace or error traces.
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

impl<T> From<T> for OhnoCore
where
    T: Into<Box<dyn StdError + Send + Sync>>,
{
    fn from(value: T) -> Self {
        // StringError is a private error type and cannot be referenced directly
        if is_string_error(&value) {
            Self::from_source(Source::Transparent(value.into().into()))
        } else {
            Self::from_source(Source::Error(value.into().into()))
        }
    }
}

const STR_TYPE_IDS: [typeid::ConstTypeId; 3] = [
    typeid::ConstTypeId::of::<&str>(),
    typeid::ConstTypeId::of::<String>(),
    typeid::ConstTypeId::of::<Cow<'_, str>>(),
];

fn is_string_error<T>(_: &T) -> bool {
    let typeid_of_t = typeid::of::<T>();
    STR_TYPE_IDS.iter().any(|&id| id == typeid_of_t)
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
        let error = OhnoCore::from("test error");
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
        let error = OhnoCore::from("msg");
        assert!(error.source().is_none());
        if let Source::Transparent(source) = &error.data.source {
            assert_eq!(source.to_string(), "msg");
        }
        assert!(matches!(&error.data.source, Source::Transparent(_)), "expected transparent source");
    }

    #[test]
    fn test_caused_by_without_backtrace() {
        let io_error = std::io::Error::other("io error");
        let error = OhnoCore::without_backtrace(io_error);
        assert!(matches!(error.data.source, Source::Error(_)));
        assert!(!error.has_backtrace());
        assert!(error.source().unwrap().downcast_ref::<std::io::Error>().is_some());
    }

    #[test]
    fn test_caused_by() {
        let io_error = std::io::Error::other("io error");
        let error = OhnoCore::from(io_error);
        assert!(matches!(error.data.source, Source::Error(_)));
        assert!(error.source().unwrap().downcast_ref::<std::io::Error>().is_some());
    }

    #[test]
    fn test_from_boxed_error() {
        let io_error = std::io::Error::other("io error");
        let boxed: Box<dyn StdError + Send + Sync> = Box::new(io_error);
        let error = OhnoCore::from(boxed);
        assert!(matches!(error.data.source, Source::Error(_)));
        assert!(error.source().unwrap().downcast_ref::<std::io::Error>().is_some());
    }

    #[test]
    fn test_from_boxed_error_2() {
        let io_error = std::io::Error::other("io error");
        let boxed: Box<dyn StdError + Send + Sync> = Box::new(io_error);
        let error: OhnoCore = boxed.into();
        assert!(matches!(error.data.source, Source::Error(_)));
        assert!(error.source().unwrap().downcast_ref::<std::io::Error>().is_some());
    }

    #[test]
    fn test_enrichment_iter_and_messages() {
        let mut error = OhnoCore::from("msg");
        error.add_enrichment(EnrichmentEntry::new("ctx1", "test.rs", 1));
        error.add_enrichment(EnrichmentEntry::new("ctx2", "test.rs", 2));
        let messages: Vec<_> = error.enrichment_messages().collect();
        assert_eq!(messages, vec!["ctx2", "ctx1"]);
    }

    #[test]
    fn test_display_and_debug() {
        let error = OhnoCore::from("msg");
        let display = format!("{error}");
        assert!(display.starts_with("msg"));
        let debug = format!("{error:?}");
        assert!(debug.contains("OhnoCore"));
    }

    #[test]
    fn test_from_string_impls() {
        let s = "abc";
        let error1: OhnoCore = s.into();
        assert!(error1.to_string().starts_with("abc"));
        assert!(matches!(error1.data.source, Source::Transparent(_)));

        let error2: OhnoCore = String::from("def").into();
        assert!(error2.to_string().starts_with("def"));
        assert!(matches!(error2.data.source, Source::Transparent(_)));

        let error3: OhnoCore = Cow::Borrowed("ghi").into();
        assert!(error3.to_string().starts_with("ghi"));
        assert!(matches!(error3.data.source, Source::Transparent(_)));
    }

    #[test]
    fn test_from_boxed_error_impl() {
        let io_error = std::io::Error::other("io error");
        let boxed: Box<dyn StdError + Send + Sync> = Box::new(io_error);
        let error: OhnoCore = boxed.into();
        assert!(matches!(error.data.source, Source::Error(_)));
        assert!(error.source().unwrap().downcast_ref::<std::io::Error>().is_some());
    }

    #[test]
    fn test_from_io_error_impl() {
        let io_error = std::io::Error::other("io error");
        let error: OhnoCore = io_error.into();
        assert!(matches!(error.data.source, Source::Error(_)));
        assert!(error.source().unwrap().downcast_ref::<std::io::Error>().is_some());
    }

    #[test]
    #[cfg_attr(miri, ignore)] // unsupported operation: `GetCurrentDirectoryW` not available when isolation is enabled
    fn force_backtrace_capture() {
        let mut error = OhnoCore::from("test error with backtrace");
        error.data.backtrace = Backtrace::force_capture();

        assert!(error.has_backtrace());
        let backtrace = error.backtrace();
        assert_eq!(backtrace.status(), BacktraceStatus::Captured);
        let display = format!("{error}");
        assert!(display.starts_with("test error with backtrace\n\nBacktrace:\n"));
    }

    #[test]
    fn no_backtrace_capture() {
        let mut error = OhnoCore::from("test error without backtrace");
        error.data.backtrace = Backtrace::disabled();
        assert!(!error.has_backtrace());
        assert_eq!(error.backtrace().status(), BacktraceStatus::Disabled);
        let display = format!("{error}");
        assert_eq!(display, "test error without backtrace");
    }

    #[test]
    fn is_string_error_test() {
        assert!(is_string_error(&"a string slice"));
        assert!(is_string_error(&String::from("a string")));
        assert!(is_string_error(&Cow::Borrowed("a string slice")));
        assert!(is_string_error(&Cow::<'static, str>::Owned(String::from("a string"))));
        assert!(!is_string_error(&std::io::Error::other("an io error")));
    }
}
