// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Example demonstrating how to use existing classification taxonomy with templated paths in `fetch`,

use data_privacy::{
    RedactedToString, RedactionEngine, classified,
    simple_redactor::{SimpleRedactor, SimpleRedactorMode},
    taxonomy,
};
use templated_uri::{BaseUri, Escape, EscapedString, Uri, templated};

// Example taxonomy for demonstration purposes
#[taxonomy(example_taxonomy)]
enum ExampleTaxonomy {
    /// Organizationally Identifiable Information
    Oii,
    /// End User Pseudonymous Identifier
    Eupi,
}

#[classified(ExampleTaxonomy::Oii)]
#[derive(Escape)]
struct OrgId(EscapedString);

#[classified(ExampleTaxonomy::Eupi)]
#[derive(Escape)]
struct UserId(u32);

#[templated(template = "/{org_id}/user/{user_id}/{item}/")]
struct UserPath {
    org_id: OrgId,
    user_id: UserId,
    #[unredacted]
    item: EscapedString,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let engine = RedactionEngine::builder()
        .set_fallback_redactor(SimpleRedactor::with_mode(SimpleRedactorMode::Replace('*')))
        .build();

    let user_path = UserPath {
        org_id: OrgId(EscapedString::from_static("Contosso")),
        user_id: UserId(42),
        item: EscapedString::from_static("foo"),
    };

    let target = Uri::default()
        .with_base(BaseUri::from_static("https://example.com"))
        .with_path(user_path);

    // You need to be careful with this as it contains the classified data.
    let _actual_uri: Uri = target.clone();

    // Either of these is safe for telemetry:
    println!("URI (debug): {target:?}"); // Prints safe generic debug representation
    println!(
        "URI (redacted): {}",
        target.to_redacted_string(&engine) // Prints via redactor
    );
    println!("URI (unredacted): {}", target.to_string().declassify_ref());

    Ok(())
}
