// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Lists blobs in an Azure Storage container.
//!
//! The example uses `fetch` for HTTP requests. It uses a [`Runtime`] based on
//! `anyspawn` and `tick` to run credential tasks.
//!
//! Sign in with `az` or `azd`. Set `AZURE_STORAGE_SERVICE_ENDPOINT`. Then run:
//! `cargo run -p fetch_azure --example blob_list`

use std::env;
use std::sync::Arc;

use anyspawn::Spawner;
use anyspawn_azure::Runtime;
use azure_core::credentials::TokenCredential;
use azure_core::http::{ClientOptions, Transport, Url};
use azure_identity::{DeveloperToolsCredential, DeveloperToolsCredentialOptions, Executor};
use azure_storage_blob::{BlobServiceClient, BlobServiceClientOptions};
use fetch::HttpClient as FetchClient;
use fetch_azure::HttpClient;
use futures::TryStreamExt as _;
use tick::Clock;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Skip the example if no Azure Storage endpoint is set, such as in CI.
    let Ok(endpoint) = env::var("AZURE_STORAGE_SERVICE_ENDPOINT") else {
        println!("AZURE_STORAGE_SERVICE_ENDPOINT is not set; skipping blob listing.");
        return Ok(());
    };
    let service_url: Url = endpoint.parse()?;

    // Use a tokio-based `Runtime` to run credential tools such as the Azure CLI.
    let executor: Arc<dyn Executor> = Arc::new(Runtime::new(Spawner::new_tokio(None), Clock::new_tokio()));
    let credential: Arc<dyn TokenCredential> =
        DeveloperToolsCredential::new(Some(DeveloperToolsCredentialOptions { executor: Some(executor) }))?;

    // Use a tokio `fetch` client for Azure SDK HTTP requests.
    let transport = Transport::new(HttpClient::from(FetchClient::new_tokio()).into());
    let options = BlobServiceClientOptions {
        client_options: ClientOptions {
            transport: Some(transport),
            ..Default::default()
        },
        ..Default::default()
    };

    let client = BlobServiceClient::new(service_url, Some(credential), Some(options))?.blob_container_client("examples");

    // List the blobs in the "examples" container.
    let mut pager = client.list_blobs(None)?;
    while let Some(blob) = pager.try_next().await? {
        let name = blob.name.as_deref().unwrap_or("(unknown)");
        let content_type = blob.properties.and_then(|properties| properties.content_type);
        let content_type = content_type.as_deref().unwrap_or("(unknown)");
        println!("{name} ({content_type})");
    }

    Ok(())
}
