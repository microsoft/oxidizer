// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! WinHTTP facade wiring without operating-system resources.

#![cfg(windows)]

use std::sync::LazyLock;
use bytesbuf::mem::{CallbackMemory, GlobalPool, OpaqueMemory};
use fetch::HttpClient;
use fetch_winhttp::{HttpClientWinHttpExt as _, WinHttpDeps};
use observed::Sink;
use tick::Clock;

static MEMORY: LazyLock<GlobalPool> = LazyLock::new(GlobalPool::new);

#[test]
fn facade_accepts_a_global_pool_without_building_a_client() {
    let deps = WinHttpDeps::builder(Clock::new_frozen(), MEMORY.clone(), Sink::noop()).build();
    let _builder = HttpClient::builder_winhttp(deps).minimal_pipeline();
}

/// Makes sure that the facade accepts caller-selected memory without opening a session.
///
/// The implementation's mock-backed builder tests cover allocations and client cloning.
#[test]
fn facade_accepts_memory_dependencies_without_building_a_client() {
    let memory = OpaqueMemory::new(CallbackMemory::new((), |(), min_bytes| MEMORY.reserve(min_bytes)));
    let deps = WinHttpDeps::builder(Clock::new_frozen(), memory, Sink::noop()).build();
    let _builder = HttpClient::builder_winhttp(deps).minimal_pipeline();
}
