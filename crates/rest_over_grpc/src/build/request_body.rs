// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::fmt;

/// How the HTTP request body maps onto the gRPC method's request message.
///
/// # Examples
///
/// ```
/// use rest_over_grpc::build::RequestBody;
///
/// // Defaults to consuming no body.
/// assert_eq!(RequestBody::default(), RequestBody::None);
///
/// // `Display` renders the `HttpRule.body` value.
/// assert_eq!(RequestBody::None.to_string(), "none");
/// assert_eq!(RequestBody::Whole.to_string(), "*");
/// assert_eq!(RequestBody::Field("book".to_owned()).to_string(), "book");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum RequestBody {
    /// No request body is consumed (the default for `GET`/`DELETE`).
    #[default]
    None,

    /// The entire request body maps to the whole RPC request message (`body: "*"`).
    Whole,

    /// The request body maps to a single named field of the RPC request message.
    Field(String),
}

impl fmt::Display for RequestBody {
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
        assert_eq!(RequestBody::None.to_string(), "none");
        assert_eq!(RequestBody::Whole.to_string(), "*");
        assert_eq!(RequestBody::Field("shelf".to_owned()).to_string(), "shelf");
    }
}
