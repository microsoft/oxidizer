// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::build::generator::Generator;

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
#[derive(Debug, Default)]
pub struct GeneratorBuilder {
    // Stored inverted so the derived `Default` yields output with the `tonic`
    // bridge emitted.
    no_tonic: bool,
    #[cfg(feature = "build-openapi")]
    openapi: Option<crate::build::OpenApiInfo>,
}

impl GeneratorBuilder {
    /// Sets whether, for each service, a blanket
    /// `impl <trait> for T where T: <tonic server trait>` is emitted, bridging a
    /// `tonic`-generated server so the same implementation serves REST. Enabled
    /// by default.
    #[must_use]
    pub fn emit_tonic_bridge(mut self, emit: bool) -> Self {
        self.no_tonic = !emit;
        self
    }

    /// Sets whether an OpenAPI 3.1 document is emitted, and with what
    /// metadata.
    ///
    /// `Some(info)` makes [`Generator::write`] write
    /// `{module}.openapi.json` beside the generated code; `None` (the default)
    /// emits none. Only descriptor-decoded services (via
    /// [`ServiceDefinition::from_fds`](crate::build::ServiceDefinition::from_fds)) carry
    /// the schema state to produce one.
    #[cfg(feature = "build-openapi")]
    #[must_use]
    pub fn emit_openapi_spec(mut self, info: Option<crate::build::OpenApiInfo>) -> Self {
        self.openapi = info;
        self
    }

    /// Builds the configured [`Generator`].
    #[must_use]
    pub fn build(self) -> Generator {
        Generator {
            services: Vec::new(),
            no_tonic: self.no_tonic,
            #[cfg(feature = "build-openapi")]
            openapi: self.openapi,
        }
    }
}
