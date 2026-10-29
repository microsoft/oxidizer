// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Streams an HTTP response to a file.

use std::io::{BufWriter, Write};
use std::path::PathBuf;

use fetch::HttpClient;
use futures::TryStreamExt;

#[path = "util/endpoint.rs"]
mod endpoint;

#[tokio::main]
async fn main() -> Result<(), ohno::AppError> {
    let endpoint = endpoint::Endpoint::new("https://example.com").await?;
    let client = HttpClient::builder_tokio(fetch::tokio::TokioDeps::default())
        .tls_options(endpoint.rustls_options()?)
        .build();
    let temporary_output = endpoint.is_automated().then(tempfile::tempdir).transpose()?;
    let output = temporary_output
        .as_ref()
        .map_or_else(|| PathBuf::from("output.txt"), |directory| directory.path().join("output.txt"));

    // Send a GET request and read the body as a stream.
    let body = client.get(endpoint.url.as_str()).fetch().await?.into_body();
    let mut stream = body.into_stream();

    let mut file = BufWriter::new(std::fs::File::create(&output)?);

    while let Some(mut chunk) = stream.try_next().await? {
        let size = chunk.len();

        // Write each chunk as it arrives.
        std::io::copy(&mut chunk, &mut file)?;
        println!("Wrote chunk to file, size: {size}");
    }

    file.flush()?;
    drop(file);
    println!("File download is complete.");

    if endpoint.is_automated() {
        assert_eq!(std::fs::read(&output)?, endpoint::RESPONSE_BODY);
    }
    std::fs::remove_file(&output)?; // Clean up the file after use.

    endpoint.finish(1).await?;
    Ok(())
}
