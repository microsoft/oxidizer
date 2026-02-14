// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Example demonstrating how to use existing classification taxonomy with templated paths in `fetch`,

use data_privacy::{RedactedToString, RedactionEngine, classified, taxonomy};
use obscuri::{BaseUri, Uri, UriFragment, UriSafeString};
use obscuri_macros::templated;

// Example taxonomy for demonstration purposes
#[taxonomy(example_taxonomy)]
enum ExampleTaxonomy {
    /// Organizationally Identifiable Information
    Oii,
    /// End User Pseudonymous Identifier
    Eupi,
}

#[classified(ExampleTaxonomy::Oii)]
#[derive(UriFragment)]
struct OrgId(UriSafeString);

#[classified(ExampleTaxonomy::Eupi)]
#[derive(UriFragment)]
struct UserId(u32);

#[templated(template = "/{org_id}/user/{user_id}/{item}/")]
struct UserPath {
    org_id: OrgId,
    user_id: UserId,
    #[unredacted]
    item: UriSafeString,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let user_path = UserPath {
        org_id: OrgId(UriSafeString::from_static("Contosso")),
        user_id: UserId(42),
        item: UriSafeString::from_static("foo"),
    };

    let target = Uri::default()
        .base_uri(BaseUri::from_uri_static("https://example.com"))
        .path_and_query(user_path);

    // You need to be careful with this as it contains the classified data.
    let _actual_uri: Uri = target.clone();

    // Either of this is safe for telemetry:
    println!("Redacted URI: {target:?}"); // Prints safe generic debug representation
    println!(
        "Redacted URI: {}",
        target.to_redacted_string(&RedactionEngine::default()) // Prints via redactor
    );

    Ok(())
}
