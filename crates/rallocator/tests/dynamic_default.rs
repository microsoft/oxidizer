// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Compile-time check that `default` selects the fallback type of the dynamic backend.

rallocator::rallocator!(backend = dynamic, default = mimalloc);

const _: &rallocator::Rallocator<rallocator::backend::Dynamic<rallocator::backend::Mimalloc>> = &GLOBAL;
