// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

pub(crate) const DEFAULT_HTTP_CLIENT_NAME: &str = "http_client";

/// `fetch.transport` value for the bundled Tokio + hyper transport.
#[cfg(all(feature = "tokio", any(feature = "rustls", feature = "native-tls")))]
pub(crate) const HYPER_ON_TOKIO_TRANSPORT_NAME: &str = "hyper-on-tokio";

/// `fetch.transport` value for the bundled fake (test) transport.
#[cfg(any(feature = "test-util", test))]
pub(crate) const FAKE_TRANSPORT_NAME: &str = "fake";
