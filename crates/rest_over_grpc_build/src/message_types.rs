// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`MessageTypes`] type.

/// The fully-qualified Rust request/response message types of a
/// [`ServiceMethod`], grouped so they can't be transposed at a call site.
///
/// # Examples
///
/// ```
/// use rest_over_grpc_build::MessageTypes;
///
/// let types = MessageTypes::new("crate::pb::GetShelfRequest", "crate::pb::Shelf");
/// assert_eq!(types.request(), "crate::pb::GetShelfRequest");
/// assert_eq!(types.response(), "crate::pb::Shelf");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MessageTypes {
    request: String,
    response: String,
}

impl MessageTypes {
    /// Creates the pair from the fully-qualified request and response type paths.
    #[must_use]
    pub fn new(request: impl Into<String>, response: impl Into<String>) -> Self {
        Self {
            request: request.into(),
            response: response.into(),
        }
    }

    /// The fully-qualified request message type.
    #[must_use]
    pub fn request(&self) -> &str {
        &self.request
    }

    /// The fully-qualified response message type.
    #[must_use]
    pub fn response(&self) -> &str {
        &self.response
    }
}

impl<R: Into<String>, S: Into<String>> From<(R, S)> for MessageTypes {
    fn from((request, response): (R, S)) -> Self {
        Self::new(request, response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_request_and_response() {
        let types = MessageTypes::new("crate::Req", "crate::Resp");
        assert_eq!(types.request(), "crate::Req");
        assert_eq!(types.response(), "crate::Resp");
        // The `From<(_, _)>` tuple conversion produces the same value.
        assert_eq!(MessageTypes::from(("crate::Req", "crate::Resp")), types);
    }
}
