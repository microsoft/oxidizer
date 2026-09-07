// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Longer form documentation for [`compressors`](crate).
//!
//! These guides cover the decisions that span several APIs, which no single item's documentation
//! can carry. They live as Markdown beside the crate so they read on GitHub as well as here.
//!
//! * [DESIGN.md] -- the user-visible policies: format selection, what is uniform across formats and
//!   what is not, how decompression is bounded, stream framing, and why the public surface is
//!   sealed.
//! * [SECURITY.md] -- the threat model for untrusted compressed data: what makes input untrusted,
//!   which resource each budget bounds and which it does not, which defaults apply to which
//!   consumption mode, what the caller still owns, and how those claims are verified.
//! * [IMPLEMENTATION.md] -- the mechanisms behind them: the pump state machine, the unsafe
//!   initialized-output contract every backend adapter must honour, engine pooling and why some
//!   engines are excluded, and the async driving rules.
//!
//! [DESIGN.md]: https://github.com/microsoft/oxidizer/blob/main/crates/compressors/docs/DESIGN.md
//! [SECURITY.md]: https://github.com/microsoft/oxidizer/blob/main/crates/compressors/docs/SECURITY.md
//! [IMPLEMENTATION.md]: https://github.com/microsoft/oxidizer/blob/main/crates/compressors/docs/IMPLEMENTATION.md
