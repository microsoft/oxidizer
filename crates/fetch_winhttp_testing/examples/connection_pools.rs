// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shows which clients share connections and which do not.
//!
//! WinHTTP's process-wide connection pool is disabled, so pooling follows the shape of the
//! `fetch` client rather than the shape of the process. Cloning a client shares its
//! connections; building a second client - even from a clone of the same builder - gives it
//! its own. Splitting a client across several pools with `multiple_pools` isolates those pools
//! from each other in the same way.
//!
//! The fixture counts accepted connections, which is what makes the difference observable.
//!
//! Run with `cargo run -p fetch_winhttp_testing --example connection_pools`.

fn main() {
    #[cfg(windows)]
    example::run();

    #[cfg(not(windows))]
    println!("fetch_winhttp is a Windows-only transport, so this example does nothing here.");
}

#[cfg(windows)]
mod example {
    use fetch::options::{ConnectionPoolOptions, PoolSelection};
    use fetch_winhttp::WinHttpTlsConfig;
    use fetch_winhttp_testing::{ResponsePlan, TestServer, client, client_builder};
    use http::Version;

    pub(super) fn run() {
        clones_share();
        separate_builds_do_not();
        multiple_pools_split_one_client();
    }

    fn clones_share() {
        let server = TestServer::http([ResponsePlan::ok("one"), ResponsePlan::ok("two")]);
        let test_client = client(&[Version::HTTP_11], WinHttpTlsConfig::default());
        let clone = test_client.client.clone();

        get(&test_client.client, &server.url("/one"));
        get(&clone, &server.url("/two"));

        println!("a client and its clone used {} connection(s)", server.finish().connections);
    }

    fn separate_builds_do_not() {
        let server = TestServer::http([ResponsePlan::ok("one"), ResponsePlan::ok("two")]);
        let (builder, _body_builder) = client_builder(&[Version::HTTP_11], WinHttpTlsConfig::default());
        let first = builder.clone().build();
        let second = builder.build();

        get(&first, &server.url("/one"));
        get(&second, &server.url("/two"));

        println!(
            "two clients built from one builder used {} connection(s)",
            server.finish().connections
        );
    }

    fn multiple_pools_split_one_client() {
        let server = TestServer::http([ResponsePlan::ok("one"), ResponsePlan::ok("two"), ResponsePlan::ok("three")]);
        let (builder, _body_builder) = client_builder(&[Version::HTTP_11], WinHttpTlsConfig::default());
        let split = builder
            .connection_pool_options(ConnectionPoolOptions::default().multiple_pools(2, PoolSelection::round_robin()))
            .build();

        for path in ["/one", "/two", "/three"] {
            get(&split, &server.url(path));
        }

        println!(
            "three requests round-robined across two pools used {} connection(s)",
            server.finish().connections
        );
    }

    fn get(client: &fetch::HttpClient, url: &str) {
        let body = futures::executor::block_on(client.get(url).fetch_text_body()).expect("the fixture answers every request");
        debug_assert!(!body.is_empty());
    }
}
