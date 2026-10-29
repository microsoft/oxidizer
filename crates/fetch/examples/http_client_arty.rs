// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Gets a web page through the Arty runtime and its Mio transport.

use arty::task::Builtins;
use fetch::HttpClient;

#[arty::main]
async fn main(cx: Builtins) -> Result<(), ohno::AppError> {
    let client = HttpClient::new_arty(&cx);
    let response = client.get("https://example.com").fetch().await?;
    println!("Request completed with status: {}", response.status());
    Ok(())
}
