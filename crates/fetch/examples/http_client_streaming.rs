// Copyright (c) Microsoft Corporation.

//! This example demonstrates how to use the `HttpClient` to download a response in a streaming
//! manner and write it incrementally to a file.

use std::io::{BufWriter, Write};

use fetch::HttpClient;
use futures::TryStreamExt;

#[tokio::main]
async fn main() -> Result<(), ohno::AppError> {
    let client = HttpClient::new_tokio();

    // Fetch a simple GET request and convert the response body into a stream.
    let body = client.get("https://example.com").fetch().await?.into_body();
    let mut stream = body.into_stream();

    let mut file = BufWriter::new(std::fs::File::create("output.txt")?);

    while let Some(mut chunk) = stream.try_next().await? {
        let size = chunk.len();

        // Process each chunk as it arrives.
        std::io::copy(&mut chunk, &mut file)?;
        println!("Chunk stored to a file, size: {size}");
    }

    file.flush()?;
    println!("File download completed.");

    std::fs::remove_file("output.txt")?; // Clean up the file after use.

    Ok(())
}
