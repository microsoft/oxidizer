// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

pub trait Codec<T1, T2>: Send + Sync {
    type Error: Into<Box<dyn std::error::Error + Send + Sync>>;

    fn map(&self, value: &T1) -> Result<T2, Self::Error>;
}
