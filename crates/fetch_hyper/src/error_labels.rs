// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Centralized [`ErrorLabel`] constants for errors raised by this crate.

#![allow(
    dead_code,
    reason = "labels are exhaustively defined for the surface area even if some are only used under specific features"
)]

use ohno::ErrorLabel;

pub(crate) const LABEL_CONNECT: ErrorLabel = ErrorLabel::from_static("connect");
pub(crate) const LABEL_REQUEST_HYPER: ErrorLabel = ErrorLabel::from_static("request_hyper");
pub(crate) const LABEL_HTTP_VERSION_UNSUPPORTED: ErrorLabel = ErrorLabel::from_static("http_version_unsupported");
