// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Caller-selected memory through the public WinHTTP facade, without network requests.

#![cfg(windows)]

use bytesbuf::mem::{CallbackMemory, GlobalPool, Memory as _, OpaqueMemory};
use fetch::HttpClient;
use fetch_winhttp::{HttpClientWinHttpExt as _, WinHttpDeps};
use observed::Sink;
use tick::Clock;

/// Makes sure that the facade preserves the selected provider when clients are cloned.
#[test]
fn client_and_clone_use_the_supplied_provider() {
    // Extra capacity distinguishes this provider from a global-pool reservation for a small request.
    let extra_capacity = 4096;
    let memory = OpaqueMemory::new(CallbackMemory::new(extra_capacity, |extra_capacity, min_bytes| {
        GlobalPool::new().reserve(min_bytes + extra_capacity)
    }));
    let deps = WinHttpDeps::builder(Clock::new_frozen(), memory, Sink::noop()).build();
    let client = HttpClient::builder_winhttp(deps).minimal_pipeline().build();
    let cloned = client.clone();

    assert!(client.reserve(1).capacity() > extra_capacity);
    assert!(cloned.reserve(1).capacity() > extra_capacity);
}
