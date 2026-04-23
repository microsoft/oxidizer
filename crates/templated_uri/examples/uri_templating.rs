// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Example demonstrating the basic usage of templated URI in `fetch`
use templated_uri::{BaseUri, Uri, UriEscapedString, templated};

#[templated(template = "/{org_id}/user/{user_id}/{item}", unredacted)]
#[derive(Clone)]
struct UserPath {
    org_id: UriEscapedString,
    user_id: UriEscapedString,
    item: UriEscapedString,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let user_path = UserPath {
        org_id: UriEscapedString::from_static("Acme"),
        user_id: UriEscapedString::from_static("Will_E_Coyote"),
        item: UriEscapedString::from_static("name"),
    };
    let target = Uri::default()
        .with_base(BaseUri::from_static("https://example.com"))
        .with_path(user_path);

    let uri: http::Uri = target.try_into()?;

    println!("Constructed URI: {uri}");

    Ok(())
}
