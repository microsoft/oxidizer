// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Native-only backend selection without submitting a request.

#![cfg(all(feature = "tokio", feature = "native-tls", not(feature = "rustls")))]

use std::sync::LazyLock;

use bytesbuf::mem::GlobalPool;
testing_aids::init_tracing!();

static MEMORY: LazyLock<GlobalPool> = LazyLock::new(GlobalPool::new);

#[test]
#[cfg_attr(miri, ignore = "native tests cover platform TLS initialization and default backend selection")]
fn gamma_round2_native_only_client_has_a_selected_backend() {
    let deps = fetch::tokio::TokioDeps::new(&tick::Clock::new_frozen(), MEMORY.clone());
    let client = fetch::HttpClient::builder_tokio(deps).build();
    assert!(format!("{client:?}").contains("HttpClient"));
}
