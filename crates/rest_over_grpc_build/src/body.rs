// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`Body`] type.

use core::fmt;

/// How the HTTP request body maps onto the RPC request message
/// (`HttpRule.body`).
///
/// # Examples
///
/// ```
/// use rest_over_grpc_build::Body;
///
/// assert_eq!(Body::default(), Body::None);
/// assert_eq!(
///     Body::Field("book".to_owned()),
///     Body::Field("book".to_owned())
/// );
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum Body {
    /// No request body is consumed (the default for `GET`/`DELETE`).
    #[default]
    None,
    /// The entire request body maps to the whole RPC request message (`body: "*"`).
    Whole,
    /// The request body maps to a single named field of the RPC request message.
    Field(String),
}

impl fmt::Display for Body {
    /// Renders the `HttpRule.body` value: `none`, `*`, or the field name.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => f.write_str("none"),
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
        assert_eq!(Body::None.to_string(), "none");
        assert_eq!(Body::Whole.to_string(), "*");
        assert_eq!(Body::Field("shelf".to_owned()).to_string(), "shelf");
    }
}
