// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::fmt;
use std::backtrace::Backtrace;
use std::error::Error;
use std::io;

use http_path_template::ParseError;

/// An error produced while reading service definitions from a file descriptor set.
///
/// # Examples
///
/// ```no_run
/// # fn main() {
/// # #[cfg(feature = "build")] {
/// use rest_over_grpc::build::{DescriptorError, DescriptorOptions, ServiceDefinition};
///
/// let descriptor_set = std::fs::read("target/file_descriptor_set.bin")
///     .expect("the build script wrote a FileDescriptorSet");
/// let error: DescriptorError =
///     ServiceDefinition::from_fds(&descriptor_set, &DescriptorOptions::new())
///         .expect_err("the descriptor set is invalid in this example");
///
/// assert!(!error.to_string().is_empty());
/// # }
/// # }
/// ```
#[derive(Debug)]
pub struct DescriptorError {
    kind: DescriptorErrorKind,
    #[expect(dead_code, reason = "captured for Debug output and RUST_BACKTRACE diagnostics")]
    backtrace: Box<Backtrace>,
}

#[derive(Debug)]
enum DescriptorErrorKind {
    Decode(String),
    Malformed {
        rpc: String,
        detail: String,
    },
    NoPattern {
        rpc: String,
    },
    Streaming {
        method: String,
    },
    Template {
        rpc: String,
        pattern: String,
        source: ParseError,
    },
    UnknownField {
        rpc: String,
        kind: String,
        field: String,
        message: String,
    },
    Io(io::Error),
}

impl DescriptorError {
    fn new(kind: DescriptorErrorKind) -> Self {
        Self {
            kind,
            backtrace: Box::new(Backtrace::capture()),
        }
    }

    pub(crate) fn decode(detail: &str) -> Self {
        Self::new(DescriptorErrorKind::Decode(detail.to_owned()))
    }

    pub(crate) fn malformed(rpc: &str, detail: &str) -> Self {
        Self::new(DescriptorErrorKind::Malformed {
            rpc: rpc.to_owned(),
            detail: detail.to_owned(),
        })
    }

    pub(crate) fn no_pattern(rpc: &str) -> Self {
        Self::new(DescriptorErrorKind::NoPattern { rpc: rpc.to_owned() })
    }

    pub(crate) fn streaming(method: &str) -> Self {
        Self::new(DescriptorErrorKind::Streaming { method: method.to_owned() })
    }

    pub(crate) fn template(rpc: &str, pattern: &str, source: ParseError) -> Self {
        Self::new(DescriptorErrorKind::Template {
            rpc: rpc.to_owned(),
            pattern: pattern.to_owned(),
            source,
        })
    }

    pub(crate) fn unknown_field(rpc: &str, kind: &str, field: &str, message: &str) -> Self {
        Self::new(DescriptorErrorKind::UnknownField {
            rpc: rpc.to_owned(),
            kind: kind.to_owned(),
            field: field.to_owned(),
            message: message.to_owned(),
        })
    }

    pub(crate) fn io(source: io::Error) -> Self {
        Self::new(DescriptorErrorKind::Io(source))
    }

    /// Whether the descriptor set itself could not be decoded.
    #[must_use]
    pub fn is_decode(&self) -> bool {
        matches!(self.kind, DescriptorErrorKind::Decode(_))
    }

    /// Whether an RPC's `google.api.http` annotation was structurally malformed.
    #[must_use]
    pub fn is_malformed(&self) -> bool {
        matches!(self.kind, DescriptorErrorKind::Malformed { .. })
    }

    /// Whether an RPC's `google.api.http` annotation carried no URL pattern.
    #[must_use]
    pub fn is_missing_pattern(&self) -> bool {
        matches!(self.kind, DescriptorErrorKind::NoPattern { .. })
    }

    /// Whether an annotated method is client-streaming or bidirectional, which
    /// cannot be transcoded to REST.
    #[must_use]
    pub fn is_streaming(&self) -> bool {
        matches!(self.kind, DescriptorErrorKind::Streaming { .. })
    }

    /// Whether an RPC's path template failed to parse.
    #[must_use]
    pub fn is_invalid_template(&self) -> bool {
        matches!(self.kind, DescriptorErrorKind::Template { .. })
    }

    /// Whether a `body` / `response_body` named a field the message does not have.
    #[must_use]
    pub fn is_unknown_field(&self) -> bool {
        matches!(self.kind, DescriptorErrorKind::UnknownField { .. })
    }

    /// Whether a generated file could not be written.
    #[must_use]
    pub fn is_io(&self) -> bool {
        matches!(self.kind, DescriptorErrorKind::Io(_))
    }
}

impl fmt::Display for DescriptorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            DescriptorErrorKind::Decode(detail) => {
                write!(f, "failed to decode the descriptor set: {detail}")
            }
            DescriptorErrorKind::Malformed { rpc, detail } => {
                write!(f, "RPC `{rpc}` has a malformed http annotation: {detail}")
            }
            DescriptorErrorKind::NoPattern { rpc } => {
                write!(f, "RPC `{rpc}` http annotation has no URL pattern")
            }
            DescriptorErrorKind::Streaming { method } => {
                write!(
                    f,
                    "method `{method}` is client-streaming or bidirectional, which cannot be transcoded to REST (server-streaming is supported)"
                )
            }
            DescriptorErrorKind::Template { rpc, pattern, source } => {
                write!(f, "RPC `{rpc}` has an invalid path template `{pattern}`: {source}")
            }
            DescriptorErrorKind::UnknownField { rpc, kind, field, message } => {
                write!(
                    f,
                    "RPC `{rpc}` http annotation `{kind}` names field `{field}`, which does not exist on message `{message}`"
                )
            }
            DescriptorErrorKind::Io(source) => {
                write!(f, "failed to write the generated service code: {source}")
            }
        }
    }
}

impl Error for DescriptorError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match &self.kind {
            DescriptorErrorKind::Template { source, .. } => Some(source),
            DescriptorErrorKind::Io(source) => Some(source),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use http_path_template::{Grammar, PathTemplate};

    use super::*;

    #[test]
    fn predicates_match_only_their_own_kind() {
        let template_source = PathTemplate::parse("no-leading-slash", Grammar::default()).expect_err("invalid template");
        let errors = [
            DescriptorError::decode("bad bytes"),
            DescriptorError::malformed("Rpc", "detail"),
            DescriptorError::no_pattern("Rpc"),
            DescriptorError::streaming("pkg.Svc.Rpc"),
            DescriptorError::template("Rpc", "bad", template_source),
            DescriptorError::unknown_field("Rpc", "body", "field", "pkg.Msg"),
            DescriptorError::io(io::Error::other("disk full")),
        ];
        let predicates: [fn(&DescriptorError) -> bool; 7] = [
            DescriptorError::is_decode,
            DescriptorError::is_malformed,
            DescriptorError::is_missing_pattern,
            DescriptorError::is_streaming,
            DescriptorError::is_invalid_template,
            DescriptorError::is_unknown_field,
            DescriptorError::is_io,
        ];

        for (i, error) in errors.iter().enumerate() {
            for (j, predicate) in predicates.iter().enumerate() {
                assert_eq!(predicate(error), i == j, "error {i} against predicate {j}");
            }
        }
    }

    #[test]
    fn descriptor_error_is_send_sync_static() {
        fn assert_bounds<T: Send + Sync + 'static>() {}
        assert_bounds::<DescriptorError>();
    }

    #[test]
    fn streaming_message_distinguishes_client_and_bidirectional_from_server_streaming() {
        let message = DescriptorError::streaming("pkg.Svc.Chat").to_string();
        assert!(message.contains("client-streaming or bidirectional"), "{message}");
        assert!(message.contains("server-streaming is supported"), "{message}");
    }
}
