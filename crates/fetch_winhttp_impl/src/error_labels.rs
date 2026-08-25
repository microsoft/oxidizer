// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use ohno::ErrorLabel;

pub(crate) const ABANDONED: ErrorLabel = ErrorLabel::from_static("abandoned");
pub(crate) const CONNECT: ErrorLabel = ErrorLabel::from_static("connect");
pub(crate) const INITIALIZATION: ErrorLabel = ErrorLabel::from_static("winhttp_initialization");
pub(crate) const INVALID_REQUEST: ErrorLabel = ErrorLabel::from_static("invalid_request");
pub(crate) const REQUEST_WINHTTP: ErrorLabel = ErrorLabel::from_static("request_winhttp");
pub(crate) const TIMEOUT: ErrorLabel = ErrorLabel::from_static("timeout");
pub(crate) const TLS: ErrorLabel = ErrorLabel::from_static("tls");
