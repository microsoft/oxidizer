// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Building the handler at the bottom of the pipeline.
//!
//! [`Dispatch`] owns the transports and whatever is wrapped around them.
//! Wrapping there rather than at the pipeline is what keeps a wrapper out of
//! the way: it runs inside retries, so each attempt gets a fresh one, and below
//! the attempt logs and metrics, so those keep measuring the response as it
//! arrived on the wire.

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

    // Each pooled transport is wrapped in turn, so no pool member is left
    // behaving differently from the rest.
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

/// Decompressing response bodies when a codec is compiled in.
///
/// The two halves keep the conditional compilation in one place, so the builder
/// above reads the same either way.
#[cfg(any(
    test,
    feature = "compression-gzip",
    feature = "compression-deflate",
    feature = "compression-brotli",
    feature = "compression-zstd"
))]
mod decompression {
    use bytesbuf::mem::HasMemory;
    use compressors::Resources;
    use http_compression::{Client, Compression, CompressionLayer};
    use layered::Layer as _;

    use super::{ClientOptions, HttpBodyBuilder};
    use crate::handlers::TransportHandler;

    pub(super) type Layer = CompressionLayer<Client>;

    /// Builds the layer, or `None` when the client asked for no decompression.
    pub(super) fn layer(options: &ClientOptions, body_builder: &HttpBodyBuilder) -> Option<Layer> {
        let options = &options.decompression;
        if options.methods.is_empty() {
            return None;
        }

        let formats = options.methods.iter().map(|method| method.format()).collect::<Vec<_>>();
        let mut layer = Compression::client(body_builder.clone())
            .resources(Resources::new(body_builder.memory()))
            .decompress_responses(&formats);
        if let Some(limits) = options.limits() {
            layer = layer.limits(limits);
        }
        Some(layer)
    }

    pub(super) fn wrap(handler: TransportHandler, layer: Option<&Layer>) -> TransportHandler {
        match layer {
            Some(layer) => TransportHandler::new(layer.layer(handler)),
            None => handler,
        }
    }
}

/// The same two halves for a build that links no codec.
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

    /// There is nothing to configure, so there is nothing to carry.
    pub(super) type Layer = std::convert::Infallible;

    pub(super) const fn layer(_: &ClientOptions, _: &HttpBodyBuilder) -> Option<Layer> {
        None
    }

    pub(super) const fn wrap(handler: TransportHandler, _: Option<&Layer>) -> TransportHandler {
        handler
    }
}
