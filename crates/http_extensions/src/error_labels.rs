// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Centralized [`ErrorLabel`] constants for all errors in this crate.

use ohno::ErrorLabel;

// HTTP protocol errors
pub(crate) const LABEL_HTTP_ERROR: ErrorLabel = ErrorLabel::from_static("http_error");
pub(crate) const LABEL_URI_INVALID: ErrorLabel = ErrorLabel::from_static("uri_invalid");
pub(crate) const LABEL_HEADER_VALUE_INVALID: ErrorLabel = ErrorLabel::from_static("header_value_invalid");
pub(crate) const LABEL_METHOD_INVALID: ErrorLabel = ErrorLabel::from_static("method_invalid");
pub(crate) const LABEL_STATUS_CODE_INVALID: ErrorLabel = ErrorLabel::from_static("status_code_invalid");
pub(crate) const LABEL_RESPONSE_UNSUCCESSFUL: ErrorLabel = ErrorLabel::from_static("response_unsuccessful");
pub(crate) const LABEL_BODY_SIZE_LIMIT_REACHED: ErrorLabel = ErrorLabel::from_static("body_size_limit_reached");

// Timeout errors
pub(crate) const LABEL_RESPONSE_TIMEOUT: ErrorLabel = ErrorLabel::from_static("response_timeout");
pub(crate) const LABEL_BODY_TIMEOUT: ErrorLabel = ErrorLabel::from_static("body_timeout");

// IO errors
pub(crate) const LABEL_IO: ErrorLabel = ErrorLabel::from_static("io");

// Availability errors
pub(crate) const LABEL_UNAVAILABLE: ErrorLabel = ErrorLabel::from_static("unavailable");

// Validation errors
pub(crate) const LABEL_VALIDATION: ErrorLabel = ErrorLabel::from_static("validation");
pub(crate) const LABEL_URI_MISSING: ErrorLabel = ErrorLabel::from_static("uri_missing");
pub(crate) const LABEL_BODY_UTF8_INVALID: ErrorLabel = ErrorLabel::from_static("body_utf8_invalid");

// Body errors
pub(crate) const LABEL_BODY_CONSUMED: ErrorLabel = ErrorLabel::from_static("body_consumed");
pub(crate) const LABEL_BODY_NOT_BUFFERED: ErrorLabel = ErrorLabel::from_static("body_not_buffered");
pub(crate) const LABEL_BODY_SIZE_LIMIT: ErrorLabel = ErrorLabel::from_static("body_size_limit");

// JSON errors
#[cfg(any(feature = "json", test))]
pub(crate) const LABEL_JSON: ErrorLabel = ErrorLabel::from_static("json");
#[cfg(any(feature = "json", test))]
pub(crate) const LABEL_JSON_SERIALIZATION: ErrorLabel = ErrorLabel::from_static("json_serialization");
#[cfg(any(feature = "json", test))]
pub(crate) const LABEL_JSON_DESERIALIZATION: ErrorLabel = ErrorLabel::from_static("json_deserialization");
