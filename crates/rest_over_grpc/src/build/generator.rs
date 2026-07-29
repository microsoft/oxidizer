// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::path::Path;

use proc_macro2::TokenStream;

#[cfg(feature = "build-openapi")]
use super::OpenApiInfo;
use super::generator_builder::GeneratorBuilder;
use super::generator_output::GeneratedOutput;
use super::service_definition::{ServiceDefinition, generate_transcoder};
use super::{DescriptorError, DescriptorOptions};

/// Code-generation options applied by a [`Generator`] when rendering each
/// [`ServiceDefinition`].
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct CodegenOptions {
    /// Emit the `tonic` server-trait bridge for each service.
    pub(crate) emit_tonic: bool,
}

/// Collects [`ServiceDefinition`]s and emits generated code.
///
/// # Examples
///
/// Common `build.rs` flow:
///
/// ```no_run
/// # #[cfg(feature = "build")]
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use rest_over_grpc::build::{DescriptorOptions, Generator, ServiceDefinition};
///
/// let descriptor_set = std::fs::read("target/file_descriptor_set.bin")?;
/// let options = DescriptorOptions::new().package(".library");
///
/// Generator::new()
///     .add_all(ServiceDefinition::from_fds(&descriptor_set, &options)?)
///     .write(std::env::var("OUT_DIR")?)?;
/// # Ok(())
/// # }
/// # #[cfg(not(feature = "build"))]
/// # fn main() {}
/// ```
#[derive(Debug, Clone)]
pub struct Generator {
    pub(crate) services: Vec<ServiceDefinition>,
    pub(crate) emit_tonic: bool,
    #[cfg(feature = "build-openapi")]
    pub(crate) openapi: Option<OpenApiInfo>,
}

impl Default for Generator {
    fn default() -> Self {
        Self {
            services: Vec::new(),
            emit_tonic: true,
            #[cfg(feature = "build-openapi")]
            openapi: None,
        }
    }
}

impl Generator {
    /// Creates an empty generator with the default code-generation options: the
    /// `tonic` bridge emitted, and no OpenAPI document. Use
    /// [`builder`](Self::builder) to change either of those.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts building a generator, so the code-generation options (the `tonic`
    /// bridge and OpenAPI output) can be configured before
    /// [`GeneratorBuilder::build`] produces the [`Generator`].
    #[must_use]
    // `GeneratorBuilder::default()` and `Default::default()` are the same value
    // here (the return type is `GeneratorBuilder`), so that mutant is equivalent.
    #[cfg_attr(test, mutants::skip)]
    pub fn builder() -> GeneratorBuilder {
        GeneratorBuilder::default()
    }

    /// Adds one service definition.
    ///
    /// Returns `&mut Self` so calls can be chained.
    pub fn add(&mut self, service: ServiceDefinition) -> &mut Self {
        self.services.push(service);
        self
    }

    /// Adds every service definition from an iterator.
    ///
    /// Returns `&mut Self` so calls can be chained.
    pub fn add_all(&mut self, services: impl IntoIterator<Item = ServiceDefinition>) -> &mut Self {
        self.services.extend(services);
        self
    }

    /// The code-generation options configured on this generator.
    fn codegen_options(&self) -> CodegenOptions {
        CodegenOptions {
            emit_tonic: self.emit_tonic,
        }
    }

    /// The configured OpenAPI document metadata, if any.
    #[cfg(all(test, feature = "build-openapi"))]
    pub(crate) fn openapi_info(&self) -> Option<&OpenApiInfo> {
        self.openapi.as_ref()
    }

    /// Renders all collected services, returning the top-level `Transcoder`
    /// code and one [`GeneratedOutput`] per service.
    #[must_use]
    pub fn generate(&self) -> (TokenStream, Vec<GeneratedOutput>) {
        let options = self.codegen_options();
        let outputs = self
            .services
            .iter()
            .map(|service| {
                let service_trait = service.trait_code();
                let tonic_bridge = options.emit_tonic.then(|| service.tonic_bridge());
                let output = GeneratedOutput::new(
                    service.module_name().to_owned(),
                    service.trait_name().to_owned(),
                    service_trait,
                    tonic_bridge,
                );
                #[cfg(feature = "build-openapi")]
                let output = output.with_openapi_spec(self.openapi.as_ref().and_then(|info| service.openapi_spec(info)));
                output
            })
            .collect();
        let transcoder = generate_transcoder(&self.services);
        (transcoder, outputs)
    }

    /// Writes one `{module}.rest.rs` file per module into `out_dir` and a
    /// top-level `transcoder.rest.rs`.
    ///
    /// When OpenAPI output is enabled, it also writes a `{module}.openapi.json`
    /// for each descriptor-decoded service.
    ///
    /// Include each `{module}.rest.rs` beside the generated message and serde
    /// files for that proto package. Include `transcoder.rest.rs` at a scope
    /// where those package modules are visible. The crate-level
    /// [quick start](crate#quick-start-bridge-an-existing-tonic-service) shows the
    /// complete layout.
    ///
    /// # Errors
    ///
    /// Returns an [`std::io::Error`] if a generated file cannot be written.
    pub fn write(&self, out_dir: impl AsRef<Path>) -> std::io::Result<()> {
        let out_dir = out_dir.as_ref();
        let (transcoder, outputs) = self.generate();

        let mut by_module: Vec<(String, String)> = Vec::new();
        #[cfg(feature = "build-openapi")]
        let mut openapi_by_module: Vec<(String, Vec<String>)> = Vec::new();
        for output in outputs {
            let mut code = output.service_trait().to_string();
            if let Some(bridge) = output.tonic_bridge() {
                code.push('\n');
                code.push_str(&bridge.to_string());
            }
            if let Some((_, existing)) = by_module.iter_mut().find(|(m, _)| *m == output.module_name()) {
                existing.push('\n');
                existing.push_str(&code);
            } else {
                by_module.push((output.module_name().to_owned(), code));
            }

            #[cfg(feature = "build-openapi")]
            if let Some(spec) = output.openapi_spec() {
                if let Some((_, specs)) = openapi_by_module.iter_mut().find(|(m, _)| *m == output.module_name()) {
                    specs.push(spec.to_owned());
                } else {
                    openapi_by_module.push((output.module_name().to_owned(), vec![spec.to_owned()]));
                }
            }
        }

        for (module, code) in by_module {
            std::fs::write(out_dir.join(format!("{module}.rest.rs")), code)?;
        }

        // Services sharing a module (proto package) contribute to one document, so
        // their per-service specs are merged before writing `{module}.openapi.json`.
        #[cfg(feature = "build-openapi")]
        for (module, specs) in openapi_by_module {
            std::fs::write(out_dir.join(format!("{module}.openapi.json")), merge_openapi_docs(&specs))?;
        }

        std::fs::write(out_dir.join("transcoder.rest.rs"), transcoder.to_string())?;
        Ok(())
    }
}

/// Merges one or more per-service OpenAPI documents (pretty JSON) that share a
/// module into a single document by unioning their `paths` and
/// `components.schemas` objects. The documents share identical top-level
/// metadata (`openapi`, `info`, `servers`) since they come from one generator.
#[cfg(feature = "build-openapi")]
fn merge_openapi_docs(specs: &[String]) -> String {
    use serde_json::Value;

    let mut docs = specs
        .iter()
        .map(|spec| serde_json::from_str::<Value>(spec).expect("a generated OpenAPI document is always valid JSON"));
    let mut merged = docs.next().expect("write only records a module with at least one spec");

    for doc in docs {
        if let (Some(into), Some(from)) = (merged["paths"].as_object_mut(), doc["paths"].as_object()) {
            into.extend(from.iter().map(|(key, value)| (key.clone(), value.clone())));
        }
        if let (Some(into), Some(from)) = (
            merged["components"]["schemas"].as_object_mut(),
            doc["components"]["schemas"].as_object(),
        ) {
            into.extend(from.iter().map(|(key, value)| (key.clone(), value.clone())));
        }
    }

    serde_json::to_string_pretty(&merged).expect("the merged OpenAPI document always serializes to JSON")
}

/// Reads the `google.api.http` annotations from an encoded `FileDescriptorSet`
/// and writes the generated REST service code into `out_dir`, one
/// `{module}.rest.rs` file per proto package.
///
/// This is the batteries-included form of the common `build.rs` flow, using the
/// default options: every annotated service is decoded and the `tonic` bridge is
/// emitted. For anything else — a package filter, disabling the `tonic` bridge,
/// or OpenAPI output — use [`Generator`] and [`ServiceDefinition::from_fds`]
/// directly. It is equivalent to:
///
/// ```no_run
/// # #[cfg(feature = "build")]
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// # use rest_over_grpc::build::{DescriptorOptions, Generator, ServiceDefinition};
/// # let descriptor_set = std::fs::read("target/file_descriptor_set.bin")?;
/// # let out_dir = std::env::var("OUT_DIR")?;
/// Generator::new()
///     .add_all(ServiceDefinition::from_fds(
///         &descriptor_set,
///         &DescriptorOptions::new(),
///     )?)
///     .write(out_dir)?;
/// # Ok(())
/// # }
/// # #[cfg(not(feature = "build"))]
/// # fn main() {}
/// ```
///
/// The descriptor set must also be used to generate the protobuf messages and
/// proto3-JSON serde implementations; this function generates only the REST
/// service trait, optional `tonic` bridge, and transcoder.
///
/// # Examples
///
/// ```no_run
/// # #[cfg(feature = "build")]
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use rest_over_grpc::build::compile_fds;
///
/// let descriptor_set = std::fs::read("target/file_descriptor_set.bin")?;
///
/// compile_fds(&descriptor_set, std::env::var("OUT_DIR")?)?;
/// # Ok(())
/// # }
/// # #[cfg(not(feature = "build"))]
/// # fn main() {}
/// ```
///
/// # Errors
///
/// Returns a [`DescriptorError`] if the descriptor bytes cannot be decoded,
/// an annotation is malformed, or a generated file cannot be written.
#[cfg(feature = "build")]
pub fn compile_fds(descriptor_set: impl AsRef<[u8]>, out_dir: impl AsRef<Path>) -> Result<(), DescriptorError> {
    let mut generator = Generator::new();
    generator.add_all(ServiceDefinition::from_fds(descriptor_set.as_ref(), &DescriptorOptions::new())?);
    generator.write(out_dir.as_ref()).map_err(DescriptorError::io)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn new_and_default_builder_share_the_defaults() {
        for generator in [Generator::new(), Generator::builder().build()] {
            let options = generator.codegen_options();
            assert!(options.emit_tonic);
            #[cfg(feature = "build-openapi")]
            assert!(generator.openapi_info().is_none());
        }
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn emit_tonic_bridge_toggles_the_tonic_option() {
        assert!(Generator::builder().emit_tonic_bridge(true).build().codegen_options().emit_tonic);
        assert!(!Generator::builder().emit_tonic_bridge(false).build().codegen_options().emit_tonic);
    }

    #[cfg(feature = "build-openapi")]
    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn emit_openapi_spec_stores_the_optional_info() {
        let none = Generator::builder().emit_openapi_spec(None).build();
        assert!(none.openapi_info().is_none());

        let some = Generator::builder()
            .emit_openapi_spec(Some(OpenApiInfo::new("Title", "v3")))
            .build();
        let info = some.openapi_info().expect("openapi info stored");
        assert_eq!(info.title(), "Title");
        assert_eq!(info.version(), "v3");
    }

    #[cfg(feature = "build-openapi")]
    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn merge_openapi_docs_unions_sections_and_tolerates_missing_ones() {
        let a = r#"{"openapi":"3.1.0","paths":{"/a":{}},"components":{"schemas":{"A":{}}}}"#.to_owned();
        let b = r#"{"openapi":"3.1.0","paths":{"/b":{}},"components":{"schemas":{"B":{}}}}"#.to_owned();
        let bare = r#"{"openapi":"3.1.0"}"#.to_owned();

        let merged = merge_openapi_docs(&[a, b, bare]);

        assert!(merged.contains("/a") && merged.contains("/b"), "{merged}");
        assert!(merged.contains("\"A\"") && merged.contains("\"B\""), "{merged}");
    }
}
