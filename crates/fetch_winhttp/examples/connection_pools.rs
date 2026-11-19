// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shows which clients share connections and which do not.
//!
//! The transport disables WinHTTP's process-wide connection pool. A client and
//! its clones share connections. Each separate client build has its own pool,
//! including a build from a cloned builder. `multiple_pools` also creates
//! isolated pools inside one client.
//!
//! The fixture counts accepted connections to show the difference.
//!
//! Run with `cargo run -p fetch_winhttp --example connection_pools`.

#[cfg(windows)]
#[tokio::main]
async fn main() -> Result<(), ohno::AppError> {
    example::run().await
}

#[cfg(not(windows))]
fn main() {
    println!("fetch_winhttp is a Windows-only transport, so this example does nothing here.");
}

#[cfg(windows)]
mod example {
    use fetch::options::{ConnectionPoolOptions, PoolSelection};
    use fetch_winhttp::WinHttpTlsConfig;
    use fetch_winhttp_impl::testing::{ResponsePlan, TestServer, client, client_builder};
    use http::Version;
    use tick::Clock;

    pub(super) async fn run() -> Result<(), ohno::AppError> {
        clones_share().await?;
        separate_builds_do_not().await?;
        multiple_pools_split_one_client().await?;
        Ok(())
    }

    async fn clones_share() -> Result<(), ohno::AppError> {
        let server = TestServer::http([ResponsePlan::ok("one"), ResponsePlan::ok("two")]);
        let test_client = client(&[Version::HTTP_11], WinHttpTlsConfig::default(), Clock::new_tokio());
        let clone = test_client.client.clone();

        get(&test_client.client, &server.url("/one")).await?;
        get(&clone, &server.url("/two")).await?;

        println!("a client and its clone used {} connection(s)", server.finish().connections);
        Ok(())
    }

    async fn separate_builds_do_not() -> Result<(), ohno::AppError> {
        let server = TestServer::http([ResponsePlan::ok("one"), ResponsePlan::ok("two")]);
        let (builder, _body_builder) = client_builder(&[Version::HTTP_11], WinHttpTlsConfig::default(), Clock::new_tokio());
        let first = builder.clone().build();
        let second = builder.build();

        get(&first, &server.url("/one")).await?;
        get(&second, &server.url("/two")).await?;

        println!(
            "two clients built from one builder used {} connection(s)",
            server.finish().connections
        );
        Ok(())
    }

    async fn multiple_pools_split_one_client() -> Result<(), ohno::AppError> {
        let server = TestServer::http([ResponsePlan::ok("one"), ResponsePlan::ok("two"), ResponsePlan::ok("three")]);
        let (builder, _body_builder) = client_builder(&[Version::HTTP_11], WinHttpTlsConfig::default(), Clock::new_tokio());
        let split = builder
            .connection_pool_options(ConnectionPoolOptions::default().multiple_pools(2, PoolSelection::round_robin()))
            .build();

        for path in ["/one", "/two", "/three"] {
            get(&split, &server.url(path)).await?;
        }

        println!(
            "three requests round-robined across two pools used {} connection(s)",
            server.finish().connections
        );
        Ok(())
    }

    async fn get(client: &fetch::HttpClient, url: &str) -> Result<(), ohno::AppError> {
        let body = client.get(url).fetch_text_body().await?;
        debug_assert!(!body.is_empty());
        Ok(())
    }
}
