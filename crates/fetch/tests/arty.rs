// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public behavior for the Arty runtime integration.

testing_aids::init_tracing!();

use arty::task::Builtins;
use bytesbuf::mem::GlobalPool;
use fetch::HttpClient;
use fetch::arty::ArtyDeps;
use static_assertions::{assert_impl_all, assert_not_impl_any};

assert_impl_all!(ArtyDeps: Clone, Send, Sync, std::fmt::Debug);
assert_not_impl_any!(ArtyDeps: Copy);

#[arty::test]
async fn public_constructors_build_clients(builtins: Builtins) {
    let deps = ArtyDeps::from_builtins(&builtins, GlobalPool::new());
    let _builder = HttpClient::builder_arty(deps.clone());
    let _client = HttpClient::new_arty(deps);
    let _client_from_builtins = HttpClient::new_arty(&builtins);
}

#[arty::test]
async fn explicit_dependencies_build_clients(builtins: Builtins) {
    let deps = ArtyDeps {
        clock: builtins.clock().clone(),
        scheduler: builtins.scheduler().clone(),
        io: builtins.io().clone(),
        global_pool: GlobalPool::new(),
    };
    let _client = HttpClient::new_arty(deps);
}
