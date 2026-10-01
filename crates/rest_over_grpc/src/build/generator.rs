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
    #[cfg(all(test, not(miri), feature = "build-openapi"))]
    pub(crate) fn openapi_info(&self) -> Option<&OpenApiInfo> {
        self.openapi.as_ref()
    }

    /// Renders all collected services, returning the top-level `Transcoder`
    /// code and one [`GeneratedOutput`] per service.
    ///
    /// # Errors
    ///
    /// Returns an error if two REST operations in a service conflict in the
    /// optional OpenAPI document.
    pub fn generate(&self) -> std::io::Result<(TokenStream, Vec<GeneratedOutput>)> {
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
                let output = output.with_openapi_spec(match &self.openapi {
                    Some(info) => service.openapi_spec(info)?,
                    None => None,
                });
                Ok(output)
            })
            .collect::<std::io::Result<Vec<_>>>()?;
        let transcoder = generate_transcoder(&self.services);
        Ok((transcoder, outputs))
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
    /// Returns an [`std::io::Error`] if a generated file cannot be written,
    /// or two services contribute conflicting OpenAPI operations or schemas.
    pub fn write(&self, out_dir: impl AsRef<Path>) -> std::io::Result<()> {
        self.write_with(out_dir, &mut |path, bytes| std::fs::write(path, bytes))
    }

    /// Separates rendering from file I/O so write failures can be tested at
    /// each output stage without depending on filesystem permissions.
    fn write_with(&self, out_dir: impl AsRef<Path>, writer: &mut impl FnMut(&Path, &[u8]) -> std::io::Result<()>) -> std::io::Result<()> {
        let (transcoder, outputs) = self.generate()?;
        write_generated(out_dir.as_ref(), &transcoder, outputs, writer)
    }
}

fn write_generated(
    out_dir: &Path,
    transcoder: &TokenStream,
    outputs: Vec<GeneratedOutput>,
    writer: &mut impl FnMut(&Path, &[u8]) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let rendered = render_files(transcoder, outputs)?;
    for (file_name, contents) in rendered {
        writer(&out_dir.join(file_name), contents.as_bytes())?;
    }
    Ok(())
}

fn render_files(transcoder: &TokenStream, outputs: Vec<GeneratedOutput>) -> std::io::Result<Vec<(String, String)>> {
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

    let mut rendered: Vec<(String, String)> = by_module
        .into_iter()
        .map(|(module, code)| (format!("{module}.rest.rs"), code))
        .collect();
    #[cfg(feature = "build-openapi")]
    for (module, specs) in openapi_by_module {
        rendered.push((format!("{module}.openapi.json"), merge_openapi_docs(&specs)?));
    }
    rendered.push(("transcoder.rest.rs".to_owned(), transcoder.to_string()));
    Ok(rendered)
}

/// Merges one or more per-service OpenAPI documents (pretty JSON) that share a
/// module into a single document by unioning their `paths` and
/// `components.schemas` objects. The documents share identical top-level
/// metadata (`openapi`, `info`, `servers`) since they come from one generator.
#[cfg(feature = "build-openapi")]
fn merge_openapi_docs(specs: &[String]) -> std::io::Result<String> {
    use serde_json::Value;

    const OMITTED: &str = "x-rest-over-grpc-omitted-operations";

    // Each service already renders valid, pretty-printed JSON; only multiple
    // services sharing a module need the parse/merge/serialize round trip.
    if specs.len() == 1 {
        return Ok(specs[0].clone());
    }

    let mut docs = specs
        .iter()
        .map(|spec| serde_json::from_str::<Value>(spec).expect("a generated OpenAPI document is always valid JSON"));
    let mut merged = docs.next().expect("write only records a module with at least one spec");

    for doc in docs {
        if let (Some(into), Some(from)) = (merged["paths"].as_object_mut(), doc["paths"].as_object()) {
            for (path, operations) in from {
                let existing = into.entry(path).or_insert_with(|| Value::Object(serde_json::Map::default()));
                let (Some(existing), Some(operations)) = (existing.as_object_mut(), operations.as_object()) else {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("invalid OpenAPI path item {path}"),
                    ));
                };
                for (verb, operation) in operations {
                    if existing.contains_key(verb) {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            format!("conflicting OpenAPI operation {verb} {path}"),
                        ));
                    }
                    existing.insert(verb.clone(), operation.clone());
                }
            }
        }
        if let (Some(into), Some(from)) = (
            merged["components"]["schemas"].as_object_mut(),
            doc["components"]["schemas"].as_object(),
        ) {
            for (key, value) in from {
                if into.get(key).is_some_and(|existing| existing != value) {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        format!("conflicting OpenAPI schema {key}"),
                    ));
                }
                into.insert(key.clone(), value.clone());
            }
        }
        if let Some(omitted) = doc.get(OMITTED).and_then(Value::as_array) {
            merged
                .as_object_mut()
                .expect("generated OpenAPI document is an object")
                .entry(OMITTED)
                .or_insert_with(|| Value::Array(Vec::new()))
                .as_array_mut()
                .expect("generated omitted operations are an array")
                .extend(omitted.iter().cloned());
        }
    }

    Ok(serde_json::to_string_pretty(&merged).expect("the merged OpenAPI document always serializes to JSON"))
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

#[cfg(all(test, not(miri)))]
#[cfg_attr(coverage_nightly, coverage(off))]
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

        let merged = merge_openapi_docs(&[a, b, bare]).unwrap();

        assert!(merged.contains("/a") && merged.contains("/b"), "{merged}");
        assert!(merged.contains("\"A\"") && merged.contains("\"B\""), "{merged}");
    }

    #[test]
    fn write_propagates_failure_from_each_output() {
        let mut service = ServiceDefinition::new("Library", None);
        service.add_method(
            super::super::HttpRule::new(
                "Get",
                routerama::HttpMethod::GET,
                http_path_template::PathTemplate::parse("/v1/library", http_path_template::Grammar::default()).unwrap(),
            ),
            "crate::Req",
            "crate::Resp",
            None,
        );
        #[cfg(feature = "build-openapi")]
        service.set_openapi(super::super::openapi::Builder::default());
        #[cfg(not(feature = "build-openapi"))]
        let mut generator = Generator::new();
        #[cfg(feature = "build-openapi")]
        let mut generator = Generator::builder()
            .emit_openapi_spec(Some(OpenApiInfo::new("Library", "v1")))
            .build();
        generator.add(service);
        #[cfg(feature = "build-openapi")]
        let writes = 3;
        #[cfg(not(feature = "build-openapi"))]
        let writes = 2;
        for fail_at in 0..writes {
            let mut index = 0;
            let error = generator
                .write_with("target", &mut |_, _| {
                    let current = index;
                    index += 1;
                    if current == fail_at {
                        Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "injected write failure"))
                    } else {
                        Ok(())
                    }
                })
                .unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
            assert_eq!(index, fail_at + 1);
        }
    }

    #[cfg(feature = "build-openapi")]
    #[test]
    fn merge_openapi_docs_preserves_single_service_document() {
        let document = "{\n  \"openapi\": \"3.1.0\",\n  \"info\": {\"title\": \"Library\"}\n}".to_owned();
        assert_eq!(merge_openapi_docs(std::slice::from_ref(&document)).unwrap(), document);
    }

    #[cfg(feature = "build-openapi")]
    #[test]
    fn merge_openapi_docs_merges_verbs_but_rejects_duplicate_operations() {
        let get = r#"{"paths":{"/x":{"get":{"operationId":"A"}}}}"#.to_owned();
        let post = r#"{"paths":{"/x":{"post":{"operationId":"B"}}}}"#.to_owned();
        let merged: serde_json::Value = serde_json::from_str(&merge_openapi_docs(&[get.clone(), post]).unwrap()).unwrap();
        assert_eq!(merged["paths"]["/x"]["get"]["operationId"], "A");
        assert_eq!(merged["paths"]["/x"]["post"]["operationId"], "B");
        let error = merge_openapi_docs(&[get.clone(), get]).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("get /x"));
    }

    #[cfg(feature = "build-openapi")]
    #[test]
    fn conflicting_openapi_documents_are_rejected_before_any_write() {
        let spec = r#"{"paths":{"/x":{"get":{"operationId":"A"}}}}"#.to_owned();
        let outputs = ["A", "B"]
            .into_iter()
            .map(|name| {
                GeneratedOutput::new("shared".to_owned(), name.to_owned(), TokenStream::new(), None).with_openapi_spec(Some(spec.clone()))
            })
            .collect();
        let mut writes = 0;

        let error = write_generated(Path::new("target"), &TokenStream::new(), outputs, &mut |_, _| {
            writes += 1;
            Ok(())
        })
        .unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert_eq!(writes, 0);
    }

    #[test]
    fn bridges_with_matching_snake_case_names_scope_their_converters() {
        let mut first = ServiceDefinition::new("FooBar", None);
        first.set_module_name("shared");
        let mut second = ServiceDefinition::new("Foo_Bar", None);
        second.set_module_name("shared");

        let mut generated = String::new();
        Generator::new()
            .add(first)
            .add(second)
            .write_with("target", &mut |path, bytes| {
                if path.ends_with("shared.rest.rs") {
                    generated = String::from_utf8(bytes.to_vec()).unwrap();
                }
                Ok(())
            })
            .unwrap();

        let file = syn::parse_file(&generated).unwrap();
        assert!(!file.items.iter().any(|item| matches!(item, syn::Item::Fn(_))));
        let bridge_modules: Vec<_> = file
            .items
            .iter()
            .filter_map(|item| if let syn::Item::Mod(module) = item { Some(module) } else { None })
            .collect();
        assert_eq!(bridge_modules.len(), 2);
        assert_eq!(bridge_modules[0].ident, "__rest_over_grpc_bridge_FooBar");
        assert_eq!(bridge_modules[1].ident, "__rest_over_grpc_bridge_Foo_Bar");
        for module in bridge_modules {
            let items = &module.content.as_ref().unwrap().1;
            assert!(items.iter().any(|item| matches!(item, syn::Item::Struct(_))));
            assert!(items.iter().any(|item| matches!(item, syn::Item::Impl(implementation)
                if implementation.items.iter().any(|item| matches!(item, syn::ImplItem::Fn(method)
                    if method.sig.ident.to_string().contains("convert_status"))))));
        }
    }

    #[cfg(feature = "build-openapi")]
    #[test]
    fn merge_openapi_docs_rejects_invalid_path_items_and_conflicting_schemas() {
        let path = r#"{"paths":{"/x":{"get":{}}}}"#.to_owned();
        let invalid_path = r#"{"paths":{"/x":null}}"#.to_owned();
        let error = merge_openapi_docs(&[invalid_path, path]).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("/x"));

        let first = r#"{"components":{"schemas":{"R":{"type":"string"}}}}"#.to_owned();
        let second = r#"{"components":{"schemas":{"R":{"type":"integer"}}}}"#.to_owned();
        let error = merge_openapi_docs(&[first, second]).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains('R'));
    }

    #[cfg(feature = "build-openapi")]
    #[test]
    fn merge_openapi_docs_retains_explicit_omissions_from_every_service() {
        let first = r#"{"paths":{"/x":{"get":{}}},"x-rest-over-grpc-omitted-operations":[{"path":"/a/{p=**}"}]}"#.to_owned();
        let second = r#"{"paths":{"/y":{"get":{}}},"x-rest-over-grpc-omitted-operations":[{"path":"/b/{p=**}"}]}"#.to_owned();
        let merged: serde_json::Value = serde_json::from_str(&merge_openapi_docs(&[first, second]).unwrap()).unwrap();
        assert!(merged["paths"].get("/x").is_some());
        assert!(merged["paths"].get("/y").is_some());
        let omissions = merged["x-rest-over-grpc-omitted-operations"].as_array().unwrap();
        assert_eq!(omissions.len(), 2);
        assert_eq!(omissions[0]["path"], "/a/{p=**}");
        assert_eq!(omissions[1]["path"], "/b/{p=**}");
    }
}
