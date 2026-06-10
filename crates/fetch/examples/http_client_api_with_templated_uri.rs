// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This example demonstrates how to use the HTTP client with multiple REST endpoints
//! and templated URIs for more complex API interactions with multiple target paths.

use fetch::fake::FakeDeps;
use fetch::{BaseUri, HttpClient};
use ohno::AppError;
use templated_uri::EscapedString;

use crate::crates_io_mock::crates_io_fake_handler;

#[path = "util/crates_io_mock.rs"]
mod crates_io_mock;

mod api {
    use fetch::HttpClient;
    use templated_uri::{EscapedString, templated};

    type Result<T, E = ohno::AppError> = std::result::Result<T, E>;

    pub struct CratesClient {
        client: HttpClient,
    }

    impl CratesClient {
        pub(crate) fn new(client: HttpClient) -> Self {
            Self { client }
        }

        async fn fetch<T>(&self, api_endpoint: CratesApi) -> Result<T>
        where
            T: serde::de::DeserializeOwned,
        {
            let result = self
                .client
                .get(templated_uri::Uri::from(api_endpoint))
                .header("User-Agent", "http-client")
                .fetch_json::<T>()
                .await?
                .into_body();
            Ok(result)
        }

        pub async fn get_crate(&self, crate_name: EscapedString) -> Result<CrateResponse> {
            let api_endpoint = CratesApi::get_crate(crate_name);
            self.fetch(api_endpoint).await
        }

        pub async fn search_crates(&self, query: EscapedString) -> Result<CratesResponse> {
            let search_url = CratesApi::search_crates(query);
            self.fetch(search_url).await
        }
    }

    #[derive(serde::Deserialize, Debug)]
    pub struct CrateResponse {
        #[serde(rename = "crate")]
        pub model: Crate,
    }

    #[derive(serde::Deserialize, Debug)]
    pub struct CratesResponse {
        #[serde(rename = "crates")]
        pub crates: Vec<Crate>,
    }

    #[derive(serde::Deserialize, Debug)]
    pub struct Crate {
        pub name: String,
        pub downloads: u64,
        pub description: String,
    }

    #[templated]
    #[derive(Clone)]
    enum CratesApi {
        Crate(CrateUrl),
        CrateSearch(CrateSearchUrl),
    }

    impl CratesApi {
        fn get_crate(crate_name: EscapedString) -> Self {
            CrateUrl { crate_name }.into()
        }
        fn search_crates(query: EscapedString) -> Self {
            CrateSearchUrl { q: query }.into()
        }
    }

    // See http_client_telemetry.rs example for cases where URIs have classified components.
    #[templated(template = "/api/v1/crates/{crate_name}", unredacted)]
    #[derive(Clone)]
    struct CrateUrl {
        crate_name: EscapedString,
    }

    // See http_client_telemetry.rs example for cases where URIs have classified components.
    #[templated(template = "/api/v1/crates{?q}", unredacted)]
    #[derive(Clone)]
    struct CrateSearchUrl {
        q: EscapedString,
    }
}

#[tokio::main]
async fn main() -> Result<(), AppError> {
    let crate_name = EscapedString::from_static("serde");

    let client =
        HttpClient::new_fake(crates_io_fake_handler(crate_name.to_string())).with_base_uri(BaseUri::from_static("https://crates.io/"));

    let watch = FakeDeps::default().clock.stopwatch();
    let api_client = api::CratesClient::new(client);
    let crate_response = api_client.get_crate(crate_name.clone()).await?;

    println!("Single crate response:");

    println!(
        "crate: {}, downloads: {}, took: {} ms, description: '{}'",
        crate_name,
        crate_response.model.downloads,
        watch.elapsed().as_millis(),
        crate_response.model.description,
    );

    let search_response = api_client.search_crates(crate_name).await?;

    println!("Crate search response:");

    for response in &search_response.crates {
        println!(
            "crate: {}, downloads: {}, description: '{}'",
            response.name, response.downloads, response.description,
        );
    }

    Ok(())
}
