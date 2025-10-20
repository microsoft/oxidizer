// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::io::Error;

#[derive(Default, Clone)]
struct Config {}

#[rustfmt::skip]
impl Config {
    const fn new() -> Self { Self {} }
    const fn new_try() -> Result<Self, Error> { Ok(Self {}) }
    async fn new_async() -> Self { Self {} }
    async fn new_try_async() -> Result<Self, Error> { Ok(Self {}) }
}

#[fundle::bundle]
struct AppState {
    config: Config,
}

#[rustfmt::skip]
async fn run() -> Result<(), Error> {
    _ = AppState::builder().config(|_| Config::new()).build();
    _ = AppState::builder().config_async(async move |_| Config::new_async().await).await.build();
    _ = AppState::builder().config_try(|_| Config::new_try())?.build();
    _ = AppState::builder().config_try_async(async move |_| Config::new_try_async().await).await?.build();
    Ok(())
}

#[test]
fn file_compiles() {
    _ = run();
}
