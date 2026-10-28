// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Builds the handler at the end of the pipeline.
//!
//! [`Dispatch`] owns the transports and their wrappers. It wraps each transport once when it builds
//! the dispatch pool. The wrapper runs inside retries, so it processes the response from every
//! attempt. It also runs inside attempt logging and metrics. Those layers receive the response
//! after removal of its encoding metadata, and their timing excludes lazy decompression work.

use http_extensions::HttpBodyBuilder;
use opentelemetry::metrics::Meter;

use crate::custom::Transport;
use crate::handlers::{Dispatch, DispatchMode};
use crate::options::{ClientOptions, PoolIndex};

/// Builds the dispatch handler, wrapping every transport it creates.
pub(crate) fn create_dispatch_handler(
    meter: &Meter,
    options: ClientOptions,
    transport: &Transport,
    body_builder: &HttpBodyBuilder,
) -> Dispatch {
    let decompression = decompression::layer(&options, body_builder);

    // Apply the same wrapper to every transport in the pool.
    let mut create = |index: usize| {
        let handler = transport.create_transport_handler(options.clone(), meter.clone(), PoolIndex::new(index));

        decompression::wrap(handler, decompression.as_ref())
    };

    let mode = match options.transport.connection_pool.multiple_pools.clone() {
        Some((pool_count, selection)) if pool_count > 1 => {
            DispatchMode::pooled((0..pool_count).map(&mut create).collect::<Vec<_>>(), selection)
        }
        _ => DispatchMode::single(create(0)),
    };

    Dispatch::new(mode, options.transport.request_filter)
}

/// Decompresses response bodies when the build includes a compression implementation.
///
/// The two implementations keep conditional compilation out of the builder.
#[cfg(any(
    test,
    feature = "compression-gzip",
    feature = "compression-deflate",
    feature = "compression-brotli",
    feature = "compression-zstd"
))]
mod decompression {
    use http_compression::{Client, Compression, CompressionLayer};
    use layered::Layer as _;

    use super::{ClientOptions, HttpBodyBuilder};
    use crate::handlers::TransportHandler;

    pub(super) type Layer = CompressionLayer<Client>;

    /// Builds the layer, or `None` when the client asked for no decompression.
    pub(super) fn layer(options: &ClientOptions, body_builder: &HttpBodyBuilder) -> Option<Layer> {
        let options = &options.decompression;
        if options.formats.is_empty() {
            return None;
        }

        let formats = options.formats.iter().map(|format| format.format()).collect::<Vec<_>>();
        let layer = Compression::client(body_builder.clone()).decompress_responses(&formats);
        let layer = layer.limits(options.limits());
        Some(layer)
    }

    pub(super) fn wrap(handler: TransportHandler, layer: Option<&Layer>) -> TransportHandler {
        match layer {
            Some(layer) => TransportHandler::new(layer.layer(handler)),
            None => handler,
        }
    }
}

/// Decompression support for a build without a compression implementation.
#[cfg(not(any(
    test,
    feature = "compression-gzip",
    feature = "compression-deflate",
    feature = "compression-brotli",
    feature = "compression-zstd"
)))]
mod decompression {
    use super::{ClientOptions, HttpBodyBuilder};
    use crate::handlers::TransportHandler;

    /// Returns an empty layer because this build cannot decompress responses.
    pub(super) type Layer = std::convert::Infallible;

    #[cfg_attr(test, mutants::skip)] // Trivial adapter, inactive in the all-features mutation suite.
    pub(super) const fn layer(_: &ClientOptions, _: &HttpBodyBuilder) -> Option<Layer> {
        None
    }

    #[cfg_attr(test, mutants::skip)] // Trivial identity adapter, inactive in the all-features mutation suite.
    pub(super) const fn wrap(handler: TransportHandler, _: Option<&Layer>) -> TransportHandler {
        handler
    }
}
