// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Timeout types for controlling HTTP request and response duration limits.

mod body_timeout;
pub use body_timeout::BodyTimeout;

mod response_timeout;
pub use response_timeout::ResponseTimeout;
