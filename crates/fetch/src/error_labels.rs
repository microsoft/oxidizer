// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(dead_code, reason = "build conditions cost more than this small shared code")]

//! Centralized [`ErrorLabel`] constants for all errors in this crate.

use ohno::ErrorLabel;

// Connection errors
pub(crate) const LABEL_CONNECT: ErrorLabel = ErrorLabel::from_static("connect");

// Request errors
pub(crate) const LABEL_REQUEST_HYPER: ErrorLabel = ErrorLabel::from_static("request_hyper");

// Handler errors
pub(crate) const LABEL_ABANDONED: ErrorLabel = ErrorLabel::from_static("abandoned");

// Validation errors
pub(crate) const LABEL_URI_ORIGIN_MISSING: ErrorLabel = ErrorLabel::from_static("uri_origin_missing");
pub(crate) const LABEL_SCHEME_NOT_ALLOWED: ErrorLabel = ErrorLabel::from_static("scheme_not_allowed");
pub(crate) const LABEL_HTTP_VERSION_UNSUPPORTED: ErrorLabel = ErrorLabel::from_static("http_version_unsupported");
