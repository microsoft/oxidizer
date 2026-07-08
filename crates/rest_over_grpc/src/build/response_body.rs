// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::fmt;

/// How the gRPC method's response message maps onto the HTTP response body.
///
/// # Examples
///
/// ```
/// use rest_over_grpc::build::ResponseBody;
///
/// // Defaults to returning the whole response message.
/// assert_eq!(ResponseBody::default(), ResponseBody::Whole);
///
/// // `Display` renders the `HttpRule.response_body` value.
/// assert_eq!(ResponseBody::Whole.to_string(), "*");
/// assert_eq!(ResponseBody::Field("book".to_owned()).to_string(), "book");
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
