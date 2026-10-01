// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::Generator;
#[cfg(feature = "build-openapi")]
use super::OpenApiInfo;

/// Builder for a [`Generator`], configuring its code-generation options.
///
/// Obtain one from [`Generator::builder`]; set any of the options below and call
/// [`build`](Self::build) to produce the [`Generator`]. Services are added to
/// the built generator (via [`Generator::add`] / [`Generator::add_all`]), not
/// here.
///
/// # Examples
///
/// ```
/// use rest_over_grpc::build::Generator;
///
/// let generator = Generator::builder().emit_tonic_bridge(true).build();
/// ```
#[derive(Debug, Clone)]
pub struct GeneratorBuilder {
    emit_tonic: bool,
    #[cfg(feature = "build-openapi")]
    openapi: Option<OpenApiInfo>,
}

impl Default for GeneratorBuilder {
    fn default() -> Self {
        Self {
            emit_tonic: true,
            #[cfg(feature = "build-openapi")]
            openapi: None,
        }
    }
}

impl GeneratorBuilder {
    /// Sets whether each service emits a
    /// `__rest_over_grpc_bridge_{Service}::{Service}RestBridge` wrapper around
    /// its `tonic`-generated server trait. Enabled by default. The consuming
    /// crate must provide the corresponding `tonic`-generated server trait.
    /// Construct the bridge with `with_guard(service, guard)` to authenticate
    /// REST request metadata before invoking a tonic method, or use
    /// `externally_authenticated(service)` only when authorization is enforced
    /// upstream of the transcoder. Tonic transport interceptors do not run on
    /// this direct trait call. Disable the bridge when implementing the
    /// generated REST trait directly or using a different gRPC stack.
    #[must_use]
    pub fn emit_tonic_bridge(mut self, emit: bool) -> Self {
        self.emit_tonic = emit;
        self
    }

    /// Sets whether an OpenAPI 3.1 document is emitted, and with what
    /// metadata.
    ///
    /// `Some(info)` makes [`Generator::write`] write `{module}.openapi.json`;
    /// `None` leaves OpenAPI output disabled. Only descriptor-decoded services
    /// carry the schema state needed to emit one. See the runnable
    /// [`openapi_document` example] for generation and consumption.
    ///
    /// [`openapi_document` example]: https://github.com/microsoft/oxidizer/blob/main/crates/rest_over_grpc_examples/examples/build/openapi_document.rs
    #[cfg(feature = "build-openapi")]
    #[cfg_attr(docsrs, doc(cfg(feature = "build-openapi")))]
    #[must_use]
    pub fn emit_openapi_spec(mut self, info: Option<OpenApiInfo>) -> Self {
        self.openapi = info;
        self
    }

    /// Builds the configured [`Generator`].
    #[must_use]
    pub fn build(self) -> Generator {
        Generator {
            services: Vec::new(),
            emit_tonic: self.emit_tonic,
            #[cfg(feature = "build-openapi")]
            openapi: self.openapi,
        }
    }
}
