// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

pub(crate) const DEFAULT_HTTP_CLIENT_NAME: &str = "http_client";

/// `fetch.runtime` value for the bundled Tokio runtime.
#[cfg(all(feature = "tokio", any(feature = "rustls", feature = "native-tls")))]
pub(crate) const TOKIO_RUNTIME_NAME: &str = "tokio";

/// `fetch.transport` value for the bundled hyper transport.
#[cfg(all(feature = "tokio", any(feature = "rustls", feature = "native-tls")))]
pub(crate) const HYPER_TRANSPORT_NAME: &str = "hyper";

/// `fetch.runtime` value for fake HTTP clients.
#[cfg(any(feature = "test-util", test))]
pub(crate) const FAKE_RUNTIME_NAME: &str = "fake";

/// `fetch.transport` value for the transport used by fake HTTP clients.
#[cfg(any(feature = "test-util", test))]
pub(crate) const FAKE_TRANSPORT_NAME: &str = "fake";
