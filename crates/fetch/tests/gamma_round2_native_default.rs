// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Native-only backend selection without submitting a request.

#![cfg(all(feature = "tokio", feature = "native-tls", not(feature = "rustls")))]

testing_aids::init_tracing!();

fn memory() -> &'static bytesbuf::mem::GlobalPool {
    static MEMORY: std::sync::OnceLock<bytesbuf::mem::GlobalPool> = std::sync::OnceLock::new();

    MEMORY.get_or_init(bytesbuf::mem::GlobalPool::new)
}

#[test]
#[cfg_attr(miri, ignore = "native tests cover platform TLS initialization and default backend selection")]
fn gamma_round2_native_only_client_has_a_selected_backend() {
    let deps = fetch::tokio::TokioDeps::new(&tick::Clock::new_frozen(), memory().clone());
    let client = fetch::HttpClient::builder_tokio(deps).build();
    assert!(format!("{client:?}").contains("HttpClient"));
}
