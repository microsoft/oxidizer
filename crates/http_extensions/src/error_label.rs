// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::Cow;
use std::collections::hash_set::HashSet;
use std::error::Error;
use std::fmt;
use std::io::ErrorKind;
use std::iter::successors;

use crate::HttpError;

/// A low-cardinality label for an [`HttpError`](crate::HttpError), useful for metrics and logging.
///
/// Wraps a [`Cow<'static, str>`] so it can hold either a static string literal
/// or a heap-allocated [`String`].
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
/// ```
/// # use http_extensions::HttpErrorLabel;
/// // From a static string
/// let label: HttpErrorLabel = "timeout".into();
/// assert_eq!(label, "timeout");
///
/// // From an owned String
/// let label: HttpErrorLabel = String::from("custom_label").into();
/// assert_eq!(label, "custom_label");
/// ```
#[derive(Clone, Eq, PartialEq, Hash, Debug, Default)]
pub struct HttpErrorLabel(Cow<'static, str>);

impl HttpErrorLabel {
    /// Creates a label by joining the parts with `.` as a separator.
    ///
    /// # Examples
    ///
    /// ```
    /// # use http_extensions::HttpErrorLabel;
    /// let label = HttpErrorLabel::from_parts(["http", "timeout"]);
    /// assert_eq!(label, "http.timeout");
    ///
    /// let label = HttpErrorLabel::from_parts(["a", "b", "c"]);
    /// assert_eq!(label, "a.b.c");
    /// ```
    #[must_use]
    pub fn from_parts(parts: impl IntoIterator<Item = impl AsRef<str>>) -> Self {
        let mut parts = parts.into_iter().filter(|v| !v.as_ref().is_empty());
        let mut result = match parts.next() {
            Some(first) => String::from(first.as_ref()),
            None => return Self::default(),
        };
        for part in parts {
            result.push('.');
            result.push_str(part.as_ref());
        }
        Self(Cow::Owned(result))
    }

    /// Creates a label by walking the error chain and joining recognized labels with `.`.
    ///
    /// Traverses the chain of [`source`](Error::source) errors starting from `error`.
    /// Each error in the chain that is recognized contributes a segment to the
    /// resulting label:
    ///
    /// - [`HttpError`](crate::HttpError) — uses its [`label`](crate::HttpError::label).
    /// - [`std::io::Error`] — uses a label derived from its [`ErrorKind`](std::io::ErrorKind)
    ///   (e.g. `connection_refused`).
    ///
    /// Duplicate labels are removed, keeping only the first occurrence. This
    /// avoids redundant segments when an outer error derives its label from
    /// an inner error (e.g. an [`HttpError`](crate::HttpError) converted from
    /// an [`std::io::Error`] shares the same [`ErrorKind`](std::io::ErrorKind)-based label).
    ///
    /// Unrecognized error types are skipped. If no error in the chain is
    /// recognized, the returned label is empty.
    ///
    /// # Examples
    ///
    /// ```
    /// # use http_extensions::{HttpError, HttpErrorLabel};
    /// # use recoverable::RecoveryInfo;
    /// // An HttpError wrapping an io::Error produces a dotted chain
    /// // of the outer HttpError label and the inner io::Error label.
    /// let io_err = std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "refused");
    /// let http_err = HttpError::other(io_err, RecoveryInfo::retry(), "connect");
    /// let label = HttpErrorLabel::from_error_chain(&http_err);
    /// assert_eq!(label, "connect.connection_refused");
    /// ```
    #[must_use]
    pub fn from_error_chain(error: &(dyn Error + 'static)) -> Self {
        // If the error has no source, return its label directly.
        if error.source().is_none() {
            return get_label_from_error(error).unwrap_or_default();
        }

        let mut seen = HashSet::new();

        let chain = successors(Some(error), |e| (*e).source())
            .filter_map(get_label_from_error)
            .filter(|label| seen.insert(label.clone()));

        Self::from_parts(chain)
    }

    /// Returns the label as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the label and returns [`Cow<'static, str>`].
    #[must_use]
    pub fn into_cow(self) -> Cow<'static, str> {
        self.0
    }
}

impl fmt::Display for HttpErrorLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl From<&'static str> for HttpErrorLabel {
    fn from(s: &'static str) -> Self {
        Self(Cow::Borrowed(s))
    }
}

impl From<String> for HttpErrorLabel {
    fn from(s: String) -> Self {
        Self(Cow::Owned(s))
    }
}

impl From<Cow<'static, str>> for HttpErrorLabel {
    fn from(s: Cow<'static, str>) -> Self {
        Self(s)
    }
}

impl From<HttpErrorLabel> for Cow<'static, str> {
    fn from(s: HttpErrorLabel) -> Self {
        s.0
    }
}

impl PartialEq<str> for HttpErrorLabel {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl PartialEq<&str> for HttpErrorLabel {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

impl AsRef<str> for HttpErrorLabel {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<ErrorKind> for HttpErrorLabel {
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
            ErrorKind::NotADirectory => "not_a_directory".into(),
            ErrorKind::IsADirectory => "is_a_directory".into(),
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
            _ => kind.to_string().replace(' ', "_").into(),
        }
    }
}

fn get_label_from_error(error: &(dyn Error + 'static)) -> Option<HttpErrorLabel> {
    if let Some(err) = error.downcast_ref::<std::io::Error>() {
        return Some(err.kind().into());
    }

    if let Some(err) = error.downcast_ref::<HttpError>() {
        return Some(err.label().clone());
    }

    None
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {

    use testing_aids::ALL_ERROR_KINDS;

    use super::*;

    #[test]
    fn from_static_str() {
        let label = HttpErrorLabel::from("static_label");
        assert_eq!(label, "static_label");
        assert_eq!(label.as_str(), "static_label");
    }

    #[test]
    fn from_string() {
        let label = HttpErrorLabel::from(String::from("owned_label"));
        assert_eq!(label, "owned_label");
        assert_eq!(label.as_str(), "owned_label");
    }

    #[test]
    fn from_cow() {
        let cow: Cow<'static, str> = Cow::Owned(String::from("cow_label"));
        let label = HttpErrorLabel::from(cow);
        assert_eq!(label, "cow_label");
    }

    #[test]
    fn display() {
        let label = HttpErrorLabel::from("display_test");
        assert_eq!(format!("{label}"), "display_test");
    }

    #[test]
    fn as_ref_str() {
        let label = HttpErrorLabel::from("as_ref_test");
        let s: &str = label.as_ref();
        assert_eq!(s, "as_ref_test");
    }

    #[test]
    fn from_parts_multiple() {
        let label = HttpErrorLabel::from_parts(["http", "client", "", "timeout"]);
        assert_eq!(label, "http.client.timeout");
    }

    #[test]
    fn from_parts_single() {
        let label = HttpErrorLabel::from_parts(["only"]);
        assert_eq!(label, "only");
    }

    #[test]
    fn from_parts_empty() {
        let label = HttpErrorLabel::from_parts(std::iter::empty::<&str>());
        assert_eq!(label, "");
    }

    #[test]
    fn from_parts_owned_strings() {
        let parts = vec![String::from("a"), String::from("b")];
        let label = HttpErrorLabel::from_parts(parts);
        assert_eq!(label, "a.b");
    }

    #[test]
    fn into_cow_borrowed() {
        let label = HttpErrorLabel::from("static_value");
        let cow = label.into_cow();
        assert!(matches!(cow, Cow::Borrowed("static_value")));
    }

    #[test]
    fn into_cow_owned() {
        let label = HttpErrorLabel::from(String::from("owned_value"));
        let cow = label.into_cow();
        assert!(matches!(cow, Cow::Owned(_)));
        assert_eq!(cow, "owned_value");
    }

    #[test]
    fn from_error_chain_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "refused");
        let label = HttpErrorLabel::from_error_chain(&io_err);
        assert_eq!(label, "connection_refused");
    }

    #[test]
    fn from_error_chain_http_error() {
        let http_err = HttpError::other(std::io::Error::other("fail"), recoverable::RecoveryInfo::never(), "my_label");
        let label = HttpErrorLabel::from_error_chain(&http_err);
        // The HttpError itself is recognized ("my_label"), plus the wrapped
        // io::Error is also recognized ("other").
        assert_eq!(label, "my_label.other");
    }

    #[test]
    fn from_error_chain_http_wrapping_io_deduplicates() {
        let io_err = std::io::Error::new(std::io::ErrorKind::BrokenPipe, "pipe");
        let http_err = HttpError::from(io_err);
        let label = HttpErrorLabel::from_error_chain(&http_err);
        // HttpError converted from io::Error gets label "broken_pipe",
        // and the inner io::Error also contributes "broken_pipe", but
        // duplicates are removed so only the first occurrence is kept.
        assert_eq!(label, "broken_pipe");
    }

    #[test]
    fn from_error_chain_keeps_distinct_labels() {
        // An HttpError with a custom label wrapping an io::Error produces
        // two distinct segments — no deduplication occurs.
        let io_err = std::io::Error::new(std::io::ErrorKind::TimedOut, "slow");
        let http_err = HttpError::other(io_err, recoverable::RecoveryInfo::retry(), "request");
        let label = HttpErrorLabel::from_error_chain(&http_err);
        assert_eq!(label, "request.timed_out");
    }

    #[test]
    fn from_error_chain_unrecognized_error() {
        // A plain string error is not recognized, so the label is empty.
        let err: Box<dyn Error + Send + Sync> = "unknown".into();
        let label = HttpErrorLabel::from_error_chain(err.as_ref());
        assert_eq!(label, "");
    }

    #[test]
    fn from_error_chain_single_http_no_source() {
        let http_err = HttpError::validation("bad input");
        let label = HttpErrorLabel::from_error_chain(&http_err);
        // validation wraps a Cow string via HttpError::other, so the inner
        // source is a plain string — only the HttpError label is recognized.
        assert_eq!(label, "validation");
    }

    #[test]
    fn error_kind_all_variants() {
        let kind_map: Vec<_> = ALL_ERROR_KINDS.iter().map(|v| (*v, HttpErrorLabel::from(*v))).collect();

        insta::assert_debug_snapshot!(kind_map);
    }
}
