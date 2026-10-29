// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Native TLS behavior for the Arty runtime integration.

testing_aids::init_tracing!();

use arty::task::Builtins;
use bytesbuf::mem::GlobalPool;
use fetch::HttpClient;
use fetch::arty::ArtyDeps;

#[arty::test]
async fn native_tls_feature_builds_arty_client(builtins: Builtins) {
    let deps = ArtyDeps::from_builtins(&builtins, GlobalPool::new());
    let _client = HttpClient::new_arty(deps);
}
