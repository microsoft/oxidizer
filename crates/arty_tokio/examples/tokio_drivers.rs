// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Uses Tokio timer and network drivers from an Arty task.

use std::time::Duration;

use arty::task::Builtins;
use arty_tokio::TokioHandle;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[arty::main]
async fn main(cx: Builtins) -> Result<(), ohno::AppError> {
    arty_tokio::init(&cx, TokioHandle::default()).await?;
    tokio::time::sleep(Duration::from_millis(10)).await;

    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let address = listener.local_addr()?;
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await?;
        let mut request = [0; 4];
        stream.read_exact(&mut request).await?;
        stream.write_all(b"pong").await?;
        Ok::<_, std::io::Error>(request)
    });

    let mut client = TcpStream::connect(address).await?;
    client.write_all(b"ping").await?;
    let mut response = [0; 4];
    client.read_exact(&mut response).await?;

    assert_eq!(server.await??, *b"ping");
    assert_eq!(response, *b"pong");
    Ok(())
}
