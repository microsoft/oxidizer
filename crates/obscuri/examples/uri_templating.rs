// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Example demonstrating the basic usage of templated URI in `fetch`
use oxidizer_rt::{Builtins, main};
use obscuri::{BaseUri, Uri, UriSafeString};
use obscuri_macros::templated;

#[templated(template = "/{org_id}/user/{user_id}/{item}", unredacted)]
#[derive(Clone)]
struct UserPath {
    org_id: UriSafeString,
    user_id: UriSafeString,
    item: UriSafeString,
}

#[main]
async fn main(_state: Builtins) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let user_path = UserPath {
        org_id: UriSafeString::from_static("Acme"),
        user_id: UriSafeString::from_static("Will_E_Coyote"),
        item: UriSafeString::from_static("name"),
    };
    let target = Uri::default()
        .base_uri(BaseUri::from_uri_static("https://example.com"))
        .path_and_query(user_path);

    let uri: http::Uri = target.try_into()?;

    println!("Constructed URI: {uri}");

    Ok(())
}
