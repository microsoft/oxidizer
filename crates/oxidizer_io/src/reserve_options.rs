// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Used by I/O endpoints to specify the optimization settings for
/// a specific I/O memory reservation made via an I/O context.
///
/// Today this is a placeholder because we have not yet implemented
/// detailed configuration of memory management options.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct ReserveOptions;