// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Streams an HTTP response to a file.

use std::io::{BufWriter, Write};
use std::path::PathBuf;

use fetch::HttpClient;
use futures::TryStreamExt;

#[tokio::main]
async fn main() -> Result<(), ohno::AppError> {
    let client = HttpClient::new_tokio();
    let temporary_output = std::env::var_os("ANVIL_EXAMPLE").is_some().then(tempfile::tempdir).transpose()?;
    let output = temporary_output
        .as_ref()
        .map_or_else(|| PathBuf::from("output.txt"), |directory| directory.path().join("output.txt"));

    // Send a GET request and read the body as a stream.
    let body = client.get("https://example.com").fetch().await?.into_body();
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

    std::fs::remove_file(&output)?; // Clean up the file after use.

    Ok(())
}
