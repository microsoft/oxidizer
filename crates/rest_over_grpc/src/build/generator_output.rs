// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use proc_macro2::TokenStream;

/// The generated artifacts for a single service, produced by
/// [`Generator::generate`](crate::build::Generator::generate).
///
/// The generated code is split into its distinct pieces:
///
/// - `r#trait` — the service trait.
/// - [`tonic_bridge`](Self::tonic_bridge) — the blanket `impl` bridging a
///   `tonic`-generated server, present only when
///   [`GeneratorBuilder::emit_tonic_bridge`](crate::build::GeneratorBuilder::emit_tonic_bridge)
///   is enabled.
/// - [`openapi_spec`](Self::openapi_spec) — the OpenAPI 3.1 document (as
///   pretty-printed JSON), present only when
///   [`GeneratorBuilder::emit_openapi_spec`](crate::build::GeneratorBuilder::emit_openapi_spec)
///   is set and the service carries OpenAPI schema state.
#[derive(Debug, Clone)]
pub struct GeneratedOutput {
    module_name: String,
    trait_name: String,
    service_trait: TokenStream,
    tonic_bridge: Option<TokenStream>,
    #[cfg(feature = "build-openapi")]
    openapi_spec: Option<String>,
}

impl GeneratedOutput {
    /// Creates a generated-service result (with no OpenAPI spec yet).
    pub(crate) fn new(module_name: String, trait_name: String, service_trait: TokenStream, tonic_bridge: Option<TokenStream>) -> Self {
        Self {
            module_name,
            trait_name,
            service_trait,
            tonic_bridge,
            #[cfg(feature = "build-openapi")]
            openapi_spec: None,
        }
    }

    /// Attaches the OpenAPI 3.1 document (pretty-printed JSON) for this service.
    #[cfg(feature = "build-openapi")]
    #[must_use]
    pub(crate) fn with_openapi_spec(mut self, spec: Option<String>) -> Self {
        self.openapi_spec = spec;
        self
    }

    /// The module the service groups under (its output file stem).
    #[must_use]
    pub fn module_name(&self) -> &str {
        &self.module_name
    }

    /// The generated service trait's name.
    #[must_use]
    pub fn trait_name(&self) -> &str {
        &self.trait_name
    }

    /// The generated service trait.
    #[must_use]
    pub fn r#trait(&self) -> &TokenStream {
        &self.service_trait
    }

    /// The blanket `impl` bridging a `tonic`-generated server, when the tonic
    /// bridge is enabled; otherwise `None`.
    #[must_use]
    pub fn tonic_bridge(&self) -> Option<&TokenStream> {
        self.tonic_bridge.as_ref()
    }

    /// The OpenAPI 3.1 document (pretty-printed JSON) for this service, when an
    /// OpenAPI spec is requested and the service carries OpenAPI schema state;
    /// otherwise `None`.
    #[cfg(feature = "build-openapi")]
    #[must_use]
    pub fn openapi_spec(&self) -> Option<&str> {
        self.openapi_spec.as_deref()
    }
}
