// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Centralized [`ErrorLabel`] constants for all errors in this crate.

use ohno::ErrorLabel;

// HTTP protocol errors
pub(crate) const LABEL_HTTP_ERROR: ErrorLabel = ErrorLabel::from_static("http_error");
pub(crate) const LABEL_INVALID_URI: ErrorLabel = ErrorLabel::from_static("invalid_uri");
pub(crate) const LABEL_INVALID_HEADER_VALUE: ErrorLabel = ErrorLabel::from_static("invalid_header_value");
pub(crate) const LABEL_INVALID_METHOD: ErrorLabel = ErrorLabel::from_static("invalid_method");
pub(crate) const LABEL_INVALID_STATUS_CODE: ErrorLabel = ErrorLabel::from_static("invalid_status_code");
pub(crate) const LABEL_UNSUCCESSFUL_RESPONSE: ErrorLabel = ErrorLabel::from_static("unsuccessful_response");
pub(crate) const LABEL_MAX_SIZE_REACHED: ErrorLabel = ErrorLabel::from_static("max_size_reached");

// Timeout errors
pub(crate) const LABEL_TIMEOUT_RESPONSE: ErrorLabel = ErrorLabel::from_static("timeout_response");
pub(crate) const LABEL_TIMEOUT_BODY: ErrorLabel = ErrorLabel::from_static("timeout_body");

// IO errors
pub(crate) const LABEL_IO: ErrorLabel = ErrorLabel::from_static("io");

// Availability errors
pub(crate) const LABEL_UNAVAILABLE: ErrorLabel = ErrorLabel::from_static("unavailable");

// Validation errors
pub(crate) const LABEL_VALIDATION: ErrorLabel = ErrorLabel::from_static("validation");
pub(crate) const LABEL_MISSING_URI: ErrorLabel = ErrorLabel::from_static("missing_uri");
pub(crate) const LABEL_INVALID_UTF8: ErrorLabel = ErrorLabel::from_static("invalid_utf8");

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
