// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`ResponseBody`] type.

use core::fmt;

/// How the RPC response message maps onto the HTTP response body
/// (`HttpRule.response_body`).
///
/// # Examples
///
/// ```
/// use rest_over_grpc_build::ResponseBody;
///
/// assert_eq!(ResponseBody::default(), ResponseBody::Whole);
/// assert_eq!(
///     ResponseBody::Field("book".to_owned()),
///     ResponseBody::Field("book".to_owned())
/// );
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum ResponseBody {
    /// The whole RPC response message is the response body (the default).
    #[default]
    Whole,
    /// A single named field of the RPC response message is the response body.
    Field(String),
}

impl fmt::Display for ResponseBody {
    /// Renders the `HttpRule.response_body` value: `*` or the field name.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Whole => f.write_str("*"),
            Self::Field(field) => f.write_str(field),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_renders_http_rule_values() {
        assert_eq!(ResponseBody::Whole.to_string(), "*");
        assert_eq!(ResponseBody::Field("book".to_owned()).to_string(), "book");
    }
}
