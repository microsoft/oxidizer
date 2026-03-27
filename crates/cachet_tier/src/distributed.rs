// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::CacheTier;

pub trait DistributedCacheTier: CacheTier<Vec<u8>, Vec<u8>> {}
