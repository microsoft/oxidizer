// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `TLS` backend selection and internal connector wiring.
//!
//! The only public symbol is [`TlsBackend`]; everything else is internal.

mod connector;
#[cfg(any(feature = "native-tls", test))]
mod native_tls;

pub(crate) use connector::TlsConnector;
