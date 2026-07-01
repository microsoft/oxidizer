// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`RuleError`] type.

use core::fmt;
use std::backtrace::Backtrace;

use http_path_template::ParseError;

/// An error produced while lowering an [`HttpRule`].
///
/// # Examples
///
/// ```
/// use rest_over_grpc_build::{HttpMethod, HttpRule};
///
/// let err = HttpRule::new("Bad", HttpMethod::Get, "no-leading-slash")
///     .lower()
///     .expect_err("a template without a leading slash is rejected");
///
/// assert_eq!(err.rpc(), "Bad");
/// assert!(err.to_string().contains("invalid path template"));
/// assert!(std::error::Error::source(&err).is_some());
/// ```
#[derive(Debug)]
pub struct RuleError {
    rpc: String,
    kind: RuleErrorKind,
    #[expect(dead_code, reason = "captured for Debug output and RUST_BACKTRACE diagnostics")]
    backtrace: Box<Backtrace>,
}

#[derive(Debug)]
enum RuleErrorKind {
    BadTemplate { pattern: String, source: ParseError },
    NestedBindings,
}

impl RuleError {
    pub(crate) fn bad_template(rpc: &str, pattern: &str, source: ParseError) -> Self {
        Self {
            rpc: rpc.to_owned(),
            kind: RuleErrorKind::BadTemplate {
                pattern: pattern.to_owned(),
                source,
            },
            backtrace: Box::new(Backtrace::capture()),
        }
    }

    pub(crate) fn nested_bindings(rpc: &str) -> Self {
        Self {
            rpc: rpc.to_owned(),
            kind: RuleErrorKind::NestedBindings,
            backtrace: Box::new(Backtrace::capture()),
        }
    }

    /// The RPC whose binding failed to lower.
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc_build::{HttpMethod, HttpRule};
    ///
    /// let err = HttpRule::new("Bad", HttpMethod::Get, "no-leading-slash")
    ///     .lower()
    ///     .expect_err("invalid path template");
    ///
    /// assert_eq!(err.rpc(), "Bad");
    /// ```
    #[must_use]
    pub fn rpc(&self) -> &str {
        &self.rpc
    }
}

impl fmt::Display for RuleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            RuleErrorKind::BadTemplate { pattern, source } => {
                write!(f, "RPC `{}` has an invalid path template `{pattern}`: {source}", self.rpc)
            }
            RuleErrorKind::NestedBindings => write!(
                f,
                "RPC `{}` has an additional_binding that itself nests additional_bindings",
                self.rpc
            ),
        }
    }
}

impl std::error::Error for RuleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.kind {
            RuleErrorKind::BadTemplate { source, .. } => Some(source),
            RuleErrorKind::NestedBindings => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::http_method::HttpMethod;
    use crate::http_rule::HttpRule;

    #[test]
    fn sources_match_kind() {
        let bad_template = HttpRule::new("Bad", HttpMethod::Get, "no-leading-slash")
            .lower()
            .expect_err("bad template");
        assert!(std::error::Error::source(&bad_template).is_some());

        let nested = HttpRule::new("Nested", HttpMethod::Get, "/nested")
            .with_additional_binding(
                HttpRule::new("Nested", HttpMethod::Get, "/nested/one").with_additional_binding(HttpRule::new(
                    "Nested",
                    HttpMethod::Get,
                    "/nested/two",
                )),
            )
            .lower()
            .expect_err("nested bindings are rejected");
        assert!(std::error::Error::source(&nested).is_none());
        assert!(nested.to_string().contains("nests additional_bindings"));
    }
}
