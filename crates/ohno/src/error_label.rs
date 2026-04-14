// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::Cow;
use std::collections::hash_set::HashSet;
use std::error::Error;
use std::fmt;
use std::io::ErrorKind;
use std::iter::successors;

/// Trait for errors that carry an [`ErrorLabel`].
pub trait Labeled {
    /// Returns the label attached to this error.
    fn label(&self) -> &ErrorLabel;
}

/// A low-cardinality label for an error, useful for metrics and logging.
///
/// # Cardinality requirements
///
/// Labels are intended for use as metric tag values and structured log fields.
/// Callers **must** ensure that every label they supply is:
///
/// - **Low-cardinality**: chosen from a small, bounded set of values known at
///   development time (e.g. `"timeout"`, `"connection_refused"`,
///   `"invalid_header"`). Dynamically-generated labels that grow without bound
///   (request IDs, timestamps, user-supplied strings, file paths, …) will cause
///   high-cardinality metric series and must be avoided.
/// - **Free of PII and high-entropy data**: labels may be exported to
///   monitoring systems and log aggregators. Never include personal
///   information, credentials, or data that could identify individual users.
///
/// Prefer `&'static str` literals whenever possible; reach for an owned
/// [`String`] only when the value is selected at runtime from a known,
/// bounded set.
///
/// # Examples
///
/// ```rust
/// # use ohno::ErrorLabel;
/// // From a static string
/// let label: ErrorLabel = "timeout".into();
/// assert_eq!(label, "timeout");
///
/// // From an owned String
/// let label: ErrorLabel = String::from("custom_label").into();
/// assert_eq!(label, "custom_label");
/// ```
#[derive(Clone, Eq, PartialEq, Hash, Debug, Default)]
pub struct ErrorLabel(Cow<'static, str>);

impl ErrorLabel {
    /// Creates a label by joining the parts with `.` as a separator.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use ohno::ErrorLabel;
    /// let label = ErrorLabel::from_parts(["http", "timeout"]);
    /// assert_eq!(label, "http.timeout");
    ///
    /// let label = ErrorLabel::from_parts(["a", "b", "c"]);
    /// assert_eq!(label, "a.b.c");
    /// ```
    #[must_use]
    pub fn from_parts(parts: impl IntoIterator<Item = impl Into<Self>>) -> Self {
        let mut parts = parts.into_iter().map(Into::into).filter(|v: &Self| !v.as_str().is_empty());
        let mut result = match parts.next() {
            Some(first) => String::from(first.as_str()),
            None => return Self::default(),
        };
        for part in parts {
            result.push('.');
            result.push_str(part.as_str());
        }
        Self(Cow::Owned(result))
    }

    /// Creates a label by walking the error chain and joining recognized labels with `.`.
    ///
    /// Traverses the chain of [`source`](Error::source) errors starting from `error`.
    /// For each error, `get_label` is called to extract an optional label. Duplicate
    /// labels are removed, keeping only the first occurrence.
    ///
    /// Unrecognized error types (where `get_label` returns `None`) are skipped. If no
    /// error in the chain is recognized, the returned label is empty.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use ohno::ErrorLabel;
    /// let io_err = std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "refused");
    /// let label = ErrorLabel::from_error_chain(&io_err, |e| {
    ///     e.downcast_ref::<std::io::Error>()
    ///         .map(|io| ErrorLabel::from(io.kind()))
    /// });
    /// assert_eq!(label, "connection_refused");
    /// ```
    #[must_use]
    pub fn from_error_chain(error: &(dyn Error + 'static), get_label: impl Fn(&(dyn Error + 'static)) -> Option<Self>) -> Self {
        // If the error has no source, return its label directly.
        if error.source().is_none() {
            return get_label(error).unwrap_or_default();
        }

        let mut seen = HashSet::new();

        let chain = successors(Some(error), |e| (*e).source())
            .filter_map(&get_label)
            .filter(|label| seen.insert(label.clone()));

        Self::from_parts(chain)
    }

    /// Returns the label as a string slice.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use ohno::ErrorLabel;
    /// let label: ErrorLabel = "timeout".into();
    /// assert_eq!(label.as_str(), "timeout");
    /// ```
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the label and returns [`Cow<'static, str>`].
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use ohno::ErrorLabel;
    /// use std::borrow::Cow;
    ///
    /// let label: ErrorLabel = "timeout".into();
    /// let cow: Cow<'static, str> = label.into_cow();
    /// assert_eq!(cow, "timeout");
    /// ```
    #[must_use]
    pub fn into_cow(self) -> Cow<'static, str> {
        self.0
    }
}

impl fmt::Display for ErrorLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl From<&'static str> for ErrorLabel {
    fn from(s: &'static str) -> Self {
        Self(Cow::Borrowed(s))
    }
}

impl From<String> for ErrorLabel {
    fn from(s: String) -> Self {
        Self(Cow::Owned(s))
    }
}

impl From<Cow<'static, str>> for ErrorLabel {
    fn from(s: Cow<'static, str>) -> Self {
        Self(s)
    }
}

impl From<ErrorLabel> for Cow<'static, str> {
    fn from(s: ErrorLabel) -> Self {
        s.into_cow()
    }
}

impl PartialEq<str> for ErrorLabel {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl PartialEq<&str> for ErrorLabel {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

impl AsRef<str> for ErrorLabel {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<ErrorKind> for ErrorLabel {
    /// Creates a label from an IO error kind.
    ///
    /// Maps each [`ErrorKind`] variant to a `snake_case` string label suitable for use as a
    /// metric tag. Unrecognized variants (e.g. future additions to [`ErrorKind`]) are converted
    /// using their [`Display`] representation with spaces replaced by underscores.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use ohno::ErrorLabel;
    /// use std::io::ErrorKind;
    ///
    /// let label = ErrorLabel::from(ErrorKind::TimedOut);
    /// assert_eq!(label, "timed_out");
    ///
    /// let label = ErrorLabel::from(ErrorKind::ConnectionRefused);
    /// assert_eq!(label, "connection_refused");
    /// ```
    #[cfg_attr(coverage_nightly, coverage(off))] // it includes unreachable variant and it's fully covered by tests
    fn from(kind: ErrorKind) -> Self {
        match kind {
            ErrorKind::NotFound => "not_found".into(),
            ErrorKind::PermissionDenied => "permission_denied".into(),
            ErrorKind::ConnectionRefused => "connection_refused".into(),
            ErrorKind::ConnectionReset => "connection_reset".into(),
            ErrorKind::HostUnreachable => "host_unreachable".into(),
            ErrorKind::NetworkUnreachable => "network_unreachable".into(),
            ErrorKind::ConnectionAborted => "connection_aborted".into(),
            ErrorKind::NotConnected => "not_connected".into(),
            ErrorKind::AddrInUse => "addr_in_use".into(),
            ErrorKind::AddrNotAvailable => "addr_not_available".into(),
            ErrorKind::NetworkDown => "network_down".into(),
            ErrorKind::BrokenPipe => "broken_pipe".into(),
            ErrorKind::AlreadyExists => "already_exists".into(),
            ErrorKind::WouldBlock => "would_block".into(),
            ErrorKind::NotADirectory => "not_directory".into(),
            ErrorKind::IsADirectory => "is_directory".into(),
            ErrorKind::DirectoryNotEmpty => "directory_not_empty".into(),
            ErrorKind::ReadOnlyFilesystem => "read_only_filesystem".into(),
            ErrorKind::StaleNetworkFileHandle => "stale_network_file_handle".into(),
            ErrorKind::InvalidInput => "invalid_input".into(),
            ErrorKind::InvalidData => "invalid_data".into(),
            ErrorKind::TimedOut => "timed_out".into(),
            ErrorKind::WriteZero => "write_zero".into(),
            ErrorKind::StorageFull => "storage_full".into(),
            ErrorKind::NotSeekable => "not_seekable".into(),
            ErrorKind::QuotaExceeded => "quota_exceeded".into(),
            ErrorKind::FileTooLarge => "file_too_large".into(),
            ErrorKind::ResourceBusy => "resource_busy".into(),
            ErrorKind::ExecutableFileBusy => "executable_file_busy".into(),
            ErrorKind::Deadlock => "deadlock".into(),
            ErrorKind::CrossesDevices => "crosses_devices".into(),
            ErrorKind::TooManyLinks => "too_many_links".into(),
            ErrorKind::InvalidFilename => "invalid_filename".into(),
            ErrorKind::ArgumentListTooLong => "argument_list_too_long".into(),
            ErrorKind::Interrupted => "interrupted".into(),
            ErrorKind::Unsupported => "unsupported".into(),
            ErrorKind::UnexpectedEof => "unexpected_eof".into(),
            ErrorKind::OutOfMemory => "out_of_memory".into(),
            ErrorKind::Other => "other".into(),
            _ => label_from_display(kind),
        }
    }
}

/// Converts a display representation of an error kind into a label by replacing spaces with
/// underscores.
fn label_from_display(display: impl fmt::Display) -> ErrorLabel {
    ErrorLabel(Cow::Owned(display.to_string().replace(' ', "_")))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {

    use testing_aids::ALL_ERROR_KINDS;

    use super::*;

    #[test]
    fn from_static_str() {
        let label = ErrorLabel::from("static_label");
        assert_eq!(label, "static_label");
        assert_eq!(label.as_str(), "static_label");
    }

    #[test]
    fn from_string() {
        let label = ErrorLabel::from(String::from("owned_label"));
        assert_eq!(label, "owned_label");
        assert_eq!(label.as_str(), "owned_label");
    }

    #[test]
    fn from_cow() {
        let cow: Cow<'static, str> = Cow::Owned(String::from("cow_label"));
        let label = ErrorLabel::from(cow);
        assert_eq!(label, "cow_label");
    }

    #[test]
    fn display() {
        let label = ErrorLabel::from("display_test");
        assert_eq!(format!("{label}"), "display_test");
    }

    #[test]
    fn as_ref_str() {
        let label = ErrorLabel::from("as_ref_test");
        let s: &str = label.as_ref();
        assert_eq!(s, "as_ref_test");
    }

    #[test]
    fn from_parts_multiple() {
        let label = ErrorLabel::from_parts(["http", "client", "", "timeout"]);
        assert_eq!(label, "http.client.timeout");
    }

    #[test]
    fn from_parts_single() {
        let label = ErrorLabel::from_parts(["only"]);
        assert_eq!(label, "only");
    }

    #[test]
    fn from_parts_empty() {
        let label = ErrorLabel::from_parts(std::iter::empty::<ErrorLabel>());
        assert_eq!(label, "");
    }

    #[test]
    fn from_parts_owned_strings() {
        let parts = vec![String::from("a"), String::from("b")];
        let label = ErrorLabel::from_parts(parts);
        assert_eq!(label, "a.b");
    }

    #[test]
    fn into_cow_borrowed() {
        let label = ErrorLabel::from("static_value");
        let cow = label.clone().into_cow();
        assert!(matches!(cow, Cow::Borrowed("static_value")));

        let cow = Cow::<'static, str>::from(label);
        assert!(matches!(cow, Cow::Borrowed("static_value")));
    }

    #[test]
    fn from_error_chain_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "refused");
        let label = ErrorLabel::from_error_chain(&io_err, io_get_label);
        assert_eq!(label, "connection_refused");
    }

    #[test]
    fn from_error_chain_unrecognized_error() {
        // A plain string error is not recognized, so the label is empty.
        let err: Box<dyn Error + Send + Sync> = "unknown".into();
        let label = ErrorLabel::from_error_chain(err.as_ref(), io_get_label);
        assert_eq!(label, "");
    }

    #[test]
    fn from_error_chain_nested_produces_dotted_label() {
        let inner = LabeledError::leaf("connection_refused");
        let outer = LabeledError::wrap("timed_out", inner);
        let label = ErrorLabel::from_error_chain(&outer, labeled_get_label);
        assert_eq!(label, "timed_out.connection_refused");
    }

    #[test]
    fn from_error_chain_deduplicates_labels() {
        let inner = LabeledError::leaf("timed_out");
        let outer = LabeledError::wrap("timed_out", inner);
        let label = ErrorLabel::from_error_chain(&outer, labeled_get_label);
        assert_eq!(label, "timed_out");
    }

    #[test]
    fn from_error_chain_skips_unrecognized_middle() {
        let innermost = LabeledError::leaf("broken_pipe");
        // Wrap in a plain string error (unrecognized by labeled_get_label), then in a labeled error.
        let middle = UnlabeledError::wrap(innermost);
        let outer = LabeledError::wrap("connection_reset", middle);
        let label = ErrorLabel::from_error_chain(&outer, labeled_get_label);
        assert_eq!(label, "connection_reset.broken_pipe");
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn error_kind_all_variants() {
        let kind_map: Vec<_> = ALL_ERROR_KINDS.iter().map(|v| (*v, ErrorLabel::from(*v))).collect();

        insta::assert_debug_snapshot!(kind_map);
    }

    #[test]
    fn label_from_display_replaces_spaces() {
        let label = label_from_display("some new error kind");
        assert_eq!(label, "some_new_error_kind");
    }

    #[test]
    fn label_from_display_no_spaces() {
        let label = label_from_display("already_snake_case");
        assert_eq!(label, "already_snake_case");
    }

    /// Test helper: extracts labels only from `std::io::Error`.
    fn io_get_label(error: &(dyn Error + 'static)) -> Option<ErrorLabel> {
        error.downcast_ref::<std::io::Error>().map(|err| err.kind().into())
    }

    fn labeled_get_label(error: &(dyn Error + 'static)) -> Option<ErrorLabel> {
        error.downcast_ref::<LabeledError>().map(|e| ErrorLabel::from(e.label))
    }

    #[derive(Debug)]
    struct LabeledError {
        label: &'static str,
        source: Option<Box<dyn Error + 'static>>,
    }

    impl LabeledError {
        fn leaf(label: &'static str) -> Self {
            Self { label, source: None }
        }

        fn wrap(label: &'static str, source: impl Error + 'static) -> Self {
            Self {
                label,
                source: Some(Box::new(source)),
            }
        }
    }

    impl fmt::Display for LabeledError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{}", self.label)
        }
    }

    impl Error for LabeledError {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            self.source.as_deref()
        }
    }

    #[derive(Debug)]
    struct UnlabeledError {
        source: Box<dyn Error + 'static>,
    }

    impl UnlabeledError {
        fn wrap(source: impl Error + 'static) -> Self {
            Self { source: Box::new(source) }
        }
    }

    impl fmt::Display for UnlabeledError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "unlabeled")
        }
    }

    impl Error for UnlabeledError {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            Some(&*self.source)
        }
    }
}
