// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This example demonstrates how to use the HTTP client with REST endpoints.

use std::borrow::Cow;

use fetch::fake::FakeDeps;
use fetch::{BaseUri, HttpClient};
use templated_uri::{EscapedString, templated};

use crate::crates_io_mock::crates_io_fake_handler;

#[path = "util/crates_io_mock.rs"]
mod crates_io_mock;

#[templated(template = "/api/v1/crates/{crate_name}", unredacted)]
#[derive(Clone)]
struct CrateUrl {
    crate_name: EscapedString,
}

#[tokio::main]
async fn main() -> Result<(), ohno::AppError> {
    let crate_name = EscapedString::from_static("serde");

    let client =
        HttpClient::new_fake(crates_io_fake_handler(crate_name.to_string())).with_base_uri(BaseUri::from_static("https://crates.io/"));

    let watch = FakeDeps::default().clock.stopwatch();

    let mut response = client
        .get(CrateUrl {
            crate_name: crate_name.clone(),
        })
        .header("User-Agent", "http-client")
        .fetch_json_ref::<CrateResponse>()
        .await?
        .into_body();

    let response = response.read()?;

    println!(
        "crate: {}, downloads: {}, took: {} ms, description: '{}'",
        crate_name,
        response.model.downloads,
        watch.elapsed().as_millis(),
        response.model.description,
    );

    Ok(())
}

#[derive(serde::Deserialize, Debug)]
struct CrateResponse<'a> {
    #[serde(rename = "crate", borrow)]
    model: Crate<'a>,
}

#[derive(serde::Deserialize, Debug)]
struct Crate<'a> {
    downloads: u64,
    #[serde(borrow)]
    description: Cow<'a, str>,
}
