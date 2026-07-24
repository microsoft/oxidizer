// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`ResponseBodyKind`] response-body mapping mode.

/// How the response message maps onto the HTTP response body (mirrors the
/// `HttpRule.response_body` field at runtime).
///
/// # Examples
///
/// ```
/// use rest_over_grpc::codegen_helpers::ResponseBodyKind;
///
/// let whole = ResponseBodyKind::Whole;
/// assert_eq!(whole, ResponseBodyKind::Whole);
///
/// let field = ResponseBodyKind::Field {
///     name: "name",
///     default: "null",
/// };
/// assert_eq!(
///     field,
///     ResponseBodyKind::Field {
///         name: "name",
///         default: "null"
///     }
/// );
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResponseBodyKind {
    /// The whole response message is the body.
    Whole,
    /// A single named top-level field of the response message is the body.
    ///
    /// `name` is the field's proto3-JSON (camelCase) name. `default` is the
    /// proto3-JSON literal to emit when the field holds its default value: the
    /// serde serialization pbjson generates omits default-valued fields, so a
    /// selected field at its default would otherwise appear absent. Emitting the
    /// default (e.g. `0`, `false`, `""`, `null`, `[]`, `{}`, or an enum name)
    /// matches the proto3-JSON representation a REST gateway returns.
    Field {
        /// The response field's proto3-JSON (camelCase) name.
        name: &'static str,
        /// The proto3-JSON literal emitted when the field is at its default.
        default: &'static str,
    },
}
