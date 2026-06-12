// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Lists blobs in an Azure Storage container using `fetch` as the transport and
//! an `anyspawn`/`tick`-backed [`Runtime`] as the credential executor.
//!
//! Set `AZURE_STORAGE_SERVICE_ENDPOINT` (and sign in with `az`/`azd`), then run:
//! `cargo run --example blob_list --features azure-identity`

use std::env;
use std::sync::Arc;

use anyspawn::Spawner;
use azure_core::credentials::TokenCredential;
use azure_core::http::{ClientOptions, Transport, Url};
use azure_identity::{DeveloperToolsCredential, DeveloperToolsCredentialOptions, Executor};
use azure_storage_blob::{BlobServiceClient, BlobServiceClientOptions};
use fetch::HttpClient as FetchClient;
use fetch_azure::{AzureHttpClient, Runtime};
use futures::TryStreamExt as _;
use tick::Clock;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let service_url: Url = env::var("AZURE_STORAGE_SERVICE_ENDPOINT")?.parse()?;

    // Run developer-credential subprocesses (e.g. the Azure CLI) on a
    // tokio-backed `Runtime` used as the credential's `Executor`.
    let executor: Arc<dyn Executor> = Arc::new(Runtime::new(Spawner::new_tokio(), Clock::new_tokio()));
    let credential: Arc<dyn TokenCredential> =
        DeveloperToolsCredential::new(Some(DeveloperToolsCredentialOptions { executor: Some(executor) }))?;

    // Use a tokio `fetch` client as the Azure SDK transport.
    let transport = Transport::new(AzureHttpClient::from(FetchClient::new_tokio()).into());
    let options = BlobServiceClientOptions {
        client_options: ClientOptions {
            transport: Some(transport),
            ..Default::default()
        },
        ..Default::default()
    };

    let client = BlobServiceClient::new(service_url, Some(credential), Some(options))?.blob_container_client("examples");

    // Enumerate blobs in the "examples" container.
    let mut pager = client.list_blobs(None)?;
    while let Some(blob) = pager.try_next().await? {
        let name = blob.name.as_deref().unwrap_or("(unknown)");
        let content_type = blob.properties.and_then(|properties| properties.content_type);
        let content_type = content_type.as_deref().unwrap_or("(unknown)");
        println!("{name} ({content_type})");
    }

    Ok(())
}
