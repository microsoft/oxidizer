// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The public [`Connect`] trait alias.

use http_extensions::Result;
use layered::Service;
use templated_uri::BaseUri;

use crate::connection::io::HyperIo;

/// A trait alias for types that establish connections to remote endpoints.
///
/// Any service that yields a hyper-compatible I/O stream from a [`BaseUri`]
/// is accepted as the transport's connector. Implemented automatically for
/// any cloneable [`Service`] with the right associated types.
pub trait Connect<S>: Service<BaseUri, Out = Result<S>> + Clone + 'static
where
    S: HyperIo,
{
}

impl<T, S> Connect<S> for T
where
    T: Service<BaseUri, Out = Result<S>> + Clone + 'static,
    S: HyperIo,
{
}
