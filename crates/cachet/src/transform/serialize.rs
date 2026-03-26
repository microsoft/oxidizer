// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::{CacheTier, TransformAdapter};

pub struct SerializeCodec;

pub struct SerializeAdapter<K, V, S>
where
    S: CacheTier<Vec<u8>, Vec<u8>>,
{
    inner: TransformAdapter<K, Vec<u8>, V, Vec<u8>, S>,
}

impl<K, V, S> SerializeAdapter<K, V, S> where S: CacheTier<Vec<u8>, Vec<u8>> {}
