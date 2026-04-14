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
///   monitoring systems and log sinks. Never include personal
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
///
/// # Character restrictions
///
/// Stored label values contain only lower-case ASCII alphanumeric characters (`a`–`z`,
/// `0`–`9`), underscores (`_`), and dots (`.`).
///
/// The [`from_static`](Self::from_static) constructor panics at compile time (or at
/// runtime if called dynamically) when given any character outside this set, including
/// upper-case letters. All other constructors silently coerce the input: upper-case
/// ASCII letters are lowered to their lower-case equivalents and every other invalid
/// character is replaced by an underscore.
#[derive(Clone, Eq, PartialEq, Hash, Debug, Default)]
pub struct ErrorLabel(Cow<'static, str>);

impl ErrorLabel {
    /// Creates a label from a static string literal.
    ///
    /// This is the preferred way to create a label from a known string at compile time.
    ///
    /// # Panics
    ///
    /// Panics if `label` contains characters other than ASCII alphanumeric, `_`, or `.`.
    /// When used in a `const` context the panic surfaces as a compile-time error.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use ohno::ErrorLabel;
    /// const TIMEOUT: ErrorLabel = ErrorLabel::from_static("timeout");
    /// assert_eq!(TIMEOUT, "timeout");
    /// ```
    ///
    /// Invalid characters cause a compile-time error:
    ///
    /// ```compile_fail
    /// # use ohno::ErrorLabel;
    /// const BAD: ErrorLabel = ErrorLabel::from_static("has space");
    /// ```
    #[must_use]
    pub const fn from_static(label: &'static str) -> Self {
        assert!(
            is_valid_label(label),
            "ErrorLabel: value must contain only lower-case ASCII alphanumeric characters, '_', or '.'"
        );

        Self(Cow::Borrowed(label))
    }

    #[must_use]
    const fn from_static_unchecked(label: &'static str) -> Self {
        debug_assert!(
            is_valid_label(label),
            "ErrorLabel: value must contain only lower-case ASCII alphanumeric characters, '_', or '.'"
        );

        Self(Cow::Borrowed(label))
    }

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
    pub fn from_error_chain(error: &(dyn Error + 'static), mut get_label: impl FnMut(&(dyn Error + 'static)) -> Option<Self>) -> Self {
        // If the error has no source, return its label directly.
        if error.source().is_none() {
            return get_label(error).unwrap_or_default();
        }

        let mut seen = HashSet::new();

        let chain = successors(Some(error), |e| (*e).source())
            .filter_map(get_label)
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

    /// Returns the value unchanged if it only contains lower-case ASCII alphanumeric,
    /// `_`, or `.` characters. Otherwise, returns an owned copy with upper-case ASCII
    /// letters lowered and every other invalid character replaced by `_`.
    fn coerce(value: Cow<'static, str>) -> Cow<'static, str> {
        if is_valid_label(&value) {
            return value;
        }

        Cow::Owned(value.chars().map(coerce_char).collect())
    }

    #[cfg_attr(coverage_nightly, coverage(off))] // it includes an unreachable variant and it's fully covered by tests
    const fn from_io(kind: ErrorKind) -> Self {
        match kind {
            ErrorKind::NotFound => Self::from_static_unchecked("not_found"),
            ErrorKind::PermissionDenied => Self::from_static_unchecked("permission_denied"),
            ErrorKind::ConnectionRefused => Self::from_static_unchecked("connection_refused"),
            ErrorKind::ConnectionReset => Self::from_static_unchecked("connection_reset"),
            ErrorKind::HostUnreachable => Self::from_static_unchecked("host_unreachable"),
            ErrorKind::NetworkUnreachable => Self::from_static_unchecked("network_unreachable"),
            ErrorKind::ConnectionAborted => Self::from_static_unchecked("connection_aborted"),
            ErrorKind::NotConnected => Self::from_static_unchecked("not_connected"),
            ErrorKind::AddrInUse => Self::from_static_unchecked("addr_in_use"),
            ErrorKind::AddrNotAvailable => Self::from_static_unchecked("addr_not_available"),
            ErrorKind::NetworkDown => Self::from_static_unchecked("network_down"),
            ErrorKind::BrokenPipe => Self::from_static_unchecked("broken_pipe"),
            ErrorKind::AlreadyExists => Self::from_static_unchecked("already_exists"),
            ErrorKind::WouldBlock => Self::from_static_unchecked("would_block"),
            ErrorKind::NotADirectory => Self::from_static_unchecked("not_directory"),
            ErrorKind::IsADirectory => Self::from_static_unchecked("is_directory"),
            ErrorKind::DirectoryNotEmpty => Self::from_static_unchecked("directory_not_empty"),
            ErrorKind::ReadOnlyFilesystem => Self::from_static_unchecked("read_only_filesystem"),
            ErrorKind::StaleNetworkFileHandle => Self::from_static_unchecked("stale_network_file_handle"),
            ErrorKind::InvalidInput => Self::from_static_unchecked("invalid_input"),
            ErrorKind::InvalidData => Self::from_static_unchecked("invalid_data"),
            ErrorKind::TimedOut => Self::from_static_unchecked("timed_out"),
            ErrorKind::WriteZero => Self::from_static_unchecked("write_zero"),
            ErrorKind::StorageFull => Self::from_static_unchecked("storage_full"),
            ErrorKind::NotSeekable => Self::from_static_unchecked("not_seekable"),
            ErrorKind::QuotaExceeded => Self::from_static_unchecked("quota_exceeded"),
            ErrorKind::FileTooLarge => Self::from_static_unchecked("file_too_large"),
            ErrorKind::ResourceBusy => Self::from_static_unchecked("resource_busy"),
            ErrorKind::ExecutableFileBusy => Self::from_static_unchecked("executable_file_busy"),
            ErrorKind::Deadlock => Self::from_static_unchecked("deadlock"),
            ErrorKind::CrossesDevices => Self::from_static_unchecked("crosses_devices"),
            ErrorKind::TooManyLinks => Self::from_static_unchecked("too_many_links"),
            ErrorKind::InvalidFilename => Self::from_static_unchecked("invalid_filename"),
            ErrorKind::ArgumentListTooLong => Self::from_static_unchecked("argument_list_too_long"),
            ErrorKind::Interrupted => Self::from_static_unchecked("interrupted"),
            ErrorKind::Unsupported => Self::from_static_unchecked("unsupported"),
            ErrorKind::UnexpectedEof => Self::from_static_unchecked("unexpected_eof"),
            ErrorKind::OutOfMemory => Self::from_static_unchecked("out_of_memory"),
            ErrorKind::Other => Self::from_static_unchecked("other"),
            _ => Self::from_static_unchecked("unknown"),
        }
    }
}

impl fmt::Display for ErrorLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl From<&'static str> for ErrorLabel {
    fn from(s: &'static str) -> Self {
        Self(Self::coerce(Cow::Borrowed(s)))
    }
}

impl From<String> for ErrorLabel {
    fn from(s: String) -> Self {
        Self(Self::coerce(Cow::Owned(s)))
    }
}

impl From<Cow<'static, str>> for ErrorLabel {
    fn from(s: Cow<'static, str>) -> Self {
        Self(Self::coerce(s))
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
    /// metric tag. Unrecognized variants (e.g. future additions to [`ErrorKind`]) are mapped
    /// to `"unknown"` to keep the label set low-cardinality.
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
    fn from(kind: ErrorKind) -> Self {
        Self::from_io(kind)
    }
}

const fn is_valid_label_char(b: u8) -> bool {
    if b.is_ascii_uppercase() {
        return false;
    }

    b.is_ascii_alphanumeric() || b == b'_' || b == b'.'
}

const fn is_valid_label(s: &str) -> bool {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if !is_valid_label_char(bytes[i]) {
            return false;
        }
        i += 1;
    }

    true
}

fn coerce_char(c: char) -> char {
    match c {
        '.' => '.',
        '_' => '_',
        c if c.is_ascii_uppercase() => c.to_ascii_lowercase(),
        c if !c.is_ascii_alphanumeric() => '_',
        c => c,
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {

    use testing_aids::ALL_ERROR_KINDS;

    use super::*;

    #[test]
    fn from_static_const() {
        const LABEL: ErrorLabel = ErrorLabel::from_static("const_label");
        assert_eq!(LABEL, "const_label");
        assert_eq!(LABEL.as_str(), "const_label");
    }

    #[test]
    fn from_static_with_dots_and_underscores() {
        const LABEL: ErrorLabel = ErrorLabel::from_static("http.timed_out");
        assert_eq!(LABEL, "http.timed_out");
    }

    #[test]
    #[should_panic(expected = "ErrorLabel: value must contain only lower-case ASCII alphanumeric characters")]
    fn from_static_panics_on_space() {
        let _ = ErrorLabel::from_static("has space");
    }

    #[test]
    #[should_panic(expected = "ErrorLabel: value must contain only lower-case ASCII alphanumeric characters")]
    fn from_static_panics_on_dash() {
        let _ = ErrorLabel::from_static("has-dash");
    }

    #[test]
    #[should_panic(expected = "ErrorLabel: value must contain only lower-case ASCII alphanumeric characters")]
    fn from_static_panics_on_uppercase() {
        let _ = ErrorLabel::from_static("HasUpper");
    }

    #[test]
    fn from_static_str() {
        let label = ErrorLabel::from("static_label");
        assert_eq!(label, "static_label");
        assert_eq!(label.as_str(), "static_label");
    }

    #[test]
    fn from_static_str_coerces_invalid_chars() {
        let label = ErrorLabel::from("has space");
        assert_eq!(label, "has_space");

        let label = ErrorLabel::from("has-dash");
        assert_eq!(label, "has_dash");

        let label = ErrorLabel::from("keep.dots_and.underscores");
        assert_eq!(label, "keep.dots_and.underscores");

        let label = ErrorLabel::from("HasUpper");
        assert_eq!(label, "hasupper");

        let label = ErrorLabel::from("MixedCase.With_PARTS");
        assert_eq!(label, "mixedcase.with_parts");
    }

    #[test]
    fn from_string() {
        let label = ErrorLabel::from(String::from("owned_label"));
        assert_eq!(label, "owned_label");
        assert_eq!(label.as_str(), "owned_label");
    }

    #[test]
    fn from_string_coerces_invalid_chars() {
        let label = ErrorLabel::from(String::from("hello world!"));
        assert_eq!(label, "hello_world_");

        let label = ErrorLabel::from(String::from("UpperCase"));
        assert_eq!(label, "uppercase");
    }

    #[test]
    fn from_cow() {
        let cow: Cow<'static, str> = Cow::Owned(String::from("cow_label"));
        let label = ErrorLabel::from(cow);
        assert_eq!(label, "cow_label");
    }

    #[test]
    fn from_cow_coerces_invalid_chars() {
        let cow: Cow<'static, str> = Cow::Borrowed("has space");
        let label = ErrorLabel::from(cow);
        assert_eq!(label, "has_space");

        let cow: Cow<'static, str> = Cow::Owned(String::from("CowUPPER"));
        let label = ErrorLabel::from(cow);
        assert_eq!(label, "cowupper");
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
