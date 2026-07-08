// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Reading `google.api.http` annotations from a compiled `FileDescriptorSet`.
//!
//! [`definitions_from_descriptor`] (behind
//! [`ServiceDefinition::from_fds`](crate::build::ServiceDefinition::from_fds))
//! decodes a `FileDescriptorSet` (as produced by `protox` / `protoc`), walks each
//! service's methods, reads the `google.api.http` method-options extension, and
//! builds one [`ServiceDefinition`](crate::build::ServiceDefinition) per service (its
//! `module` set to the proto package, or the snake-cased trait name when the
//! package is empty).
//!
//! This removes the need to hand-write [`HttpRule`](crate::build::HttpRule)s: the
//! bindings come straight from the proto annotations.
//!
//! # Type mapping
//!
//! Like `pbjson`, the generated code is meant to be `include!`d into the same
//! module as the `prost`-generated messages (beside `{package}.serde.rs`), so
//! message types are referenced by their **package-relative** Rust path — no
//! module root is supplied. A request/response message in the service's own
//! package is referenced by its simple name; a message in another package is
//! reached with `super::` hops and the intervening package modules, matching the
//! layout `prost`/`pbjson` produce.

use http_path_template::{Grammar, PathTemplate};
use prost_reflect::{
    DescriptorPool, DynamicMessage, ExtensionDescriptor, Kind, MessageDescriptor, MethodDescriptor, ServiceDescriptor, Value,
};
use routerama_build::HttpMethod;

use crate::build::binding::Binding;
use crate::build::descriptor_error::DescriptorError;
use crate::build::descriptor_options::DescriptorOptions;
use crate::build::http_rule::HttpRule;
use crate::build::request_body::RequestBody;
use crate::build::response_body::ResponseBody;
use crate::build::service_definition::{ServiceDefinition, to_snake_case};

/// Decodes every annotated service in `descriptor_set` into a
/// [`ServiceDefinition`], its `module` set to the service's proto package (or
/// the snake-cased trait name when the package is empty).
///
/// Backs [`ServiceDefinition::from_fds`]. Only services whose package is
/// covered by `options.packages()` are included (empty means all); message
/// request/response types are resolved to Rust paths relative to each service's
/// own package (after consulting `options.extern_paths()`).
///
/// # Errors
///
/// Returns a [`DescriptorError`] if the descriptor bytes cannot be decoded, a
/// method's annotation is malformed, an annotated method is client-streaming or
/// bidirectional (which cannot be transcoded), a path template fails to parse,
/// or a `body` / `response_body` names a field the request / response message
/// does not have.
pub(crate) fn definitions_from_descriptor(
    descriptor_set: &[u8],
    options: &DescriptorOptions,
) -> Result<Vec<ServiceDefinition>, DescriptorError> {
    let pool = DescriptorPool::decode(descriptor_set).map_err(|e| DescriptorError::decode(&e.to_string()))?;
    let http_ext = pool.get_extension_by_name("google.api.http");
    let prefixes = options.packages();

    let mut definitions = Vec::new();
    for service in pool.services() {
        if !matches_prefix(&service, prefixes) {
            continue;
        }

        let package = service.package_name().to_owned();
        let mut definition = ServiceDefinition::new(service.name(), service_leading_comment(&service));
        if !package.is_empty() {
            definition.module(&package);
        }
        #[cfg(feature = "build-openapi")]
        let mut openapi = crate::build::openapi::Builder::default();
        let mut has_methods = false;
        for method in service.methods() {
            let input = method.input();
            let output = method.output();
            let Some(mut rule) = read_http_rule(method.name(), &method.options(), http_ext.as_ref(), &input, &output)? else {
                continue;
            };

            // Server-streaming is transcoded to a streamed JSON body; client-
            // streaming and bidirectional RPCs have no REST mapping.
            if method.is_client_streaming() {
                return Err(DescriptorError::streaming(method.full_name()));
            }

            let request_type = relative_type_path(input.full_name(), &package, options.extern_paths());
            let response_type = relative_type_path(output.full_name(), &package, options.extern_paths());
            let doc = method_leading_comment(&method);

            // Classify every path variable that targets an `enum` field, so the
            // transcoder can accept the value by name as well as by number. Only
            // enum fields need this: scalar/`bytes`/`optional` fields are resolved
            // from their Rust type by `parse_path_field`.
            for path in rule.path_variable_field_paths() {
                if let Some(enum_type) = enum_field_rust_type(&input, &path, &package, options.extern_paths()) {
                    rule = rule.path_field_enum(path.join("."), enum_type);
                }
            }

            #[cfg(feature = "build-openapi")]
            {
                for route in &rule.clone().lower() {
                    openapi.add_operation(route, &input, &output, method.is_server_streaming());
                }
            }

            if method.is_server_streaming() {
                definition.add_server_streaming_method(rule, request_type, response_type, doc);
            } else {
                definition.add_method(rule, request_type, response_type, doc);
            }
            has_methods = true;
        }

        if has_methods {
            #[cfg(feature = "build-openapi")]
            definition.set_openapi(openapi);
            definitions.push(definition);
        }
    }

    Ok(definitions)
}

/// Returns `true` if `service`'s fully-qualified proto name is covered by one of
/// `prefixes` (matched on proto path segment boundaries, e.g. `.library` matches
/// service `.library.Library`). An empty `prefixes` slice matches every service.
pub(crate) fn matches_prefix(service: &ServiceDescriptor, prefixes: &[String]) -> bool {
    if prefixes.is_empty() {
        return true;
    }
    let full = format!(".{}", service.full_name());
    prefixes.iter().any(|prefix| {
        let prefix = format!(".{}", prefix.trim_start_matches('.'));
        full == prefix || full.starts_with(&format!("{prefix}."))
    })
}

/// Reads a method's proto leading comment from its file's `SourceCodeInfo`, if
/// present.
///
/// Source info is only populated when the descriptor set was compiled with it
/// (e.g. `protox`'s `include_source_info(true)` or `protoc`'s
/// `--include_source_info`); otherwise this returns `None` and the generated
/// method carries no doc comment. The returned comment has its trailing
/// whitespace trimmed but keeps each line's protobuf-style leading space.
fn method_leading_comment(method: &MethodDescriptor) -> Option<String> {
    // `MethodDescriptor::path` yields this method's file-relative
    // `[FileDescriptorProto.service (6), <service>, ServiceDescriptorProto.method (2), <method>]`.
    leading_comment(method.parent_service().parent_file_descriptor_proto(), method.path())
}

/// Reads a service's proto leading comment from its file's `SourceCodeInfo`, if
/// present. Behaves like [`method_leading_comment`] for the service element.
fn service_leading_comment(service: &ServiceDescriptor) -> Option<String> {
    // `ServiceDescriptor::path` yields the file-relative
    // `[FileDescriptorProto.service (6), <service>]`.
    leading_comment(service.parent_file_descriptor_proto(), service.path())
}

/// Finds the leading comment at the file-relative `SourceCodeInfo` `path` in
/// `file`, trimmed of trailing whitespace (each line keeps its protobuf-style
/// leading space), or `None` when absent or blank.
fn leading_comment(file: &prost_reflect::prost_types::FileDescriptorProto, path: &[i32]) -> Option<String> {
    let comment = file
        .source_code_info
        .as_ref()?
        .location
        .iter()
        .find(|location| location.path == path)?
        .leading_comments
        .as_deref()?;

    let trimmed = comment.trim_end();
    if trimmed.trim().is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

pub(crate) fn read_http_rule(
    rpc: &str,
    options: &DynamicMessage,
    http_ext: Option<&ExtensionDescriptor>,
    input: &MessageDescriptor,
    output: &MessageDescriptor,
) -> Result<Option<HttpRule>, DescriptorError> {
    let Some(ext) = http_ext else {
        return Ok(None);
    };
    if !options.has_extension(ext) {
        return Ok(None);
    }

    let value = options.get_extension(ext);
    let message = value
        .as_message()
        .ok_or_else(|| DescriptorError::malformed(rpc, "http annotation is not a message"))?;

    let mut rule = read_rule(rpc, message, input, output)?;

    let additional = message.get_field_by_name("additional_bindings");
    if let Some(list) = additional.as_deref().and_then(Value::as_list) {
        for entry in list {
            let entry_message = entry
                .as_message()
                .ok_or_else(|| DescriptorError::malformed(rpc, "additional_binding is not a message"))?;
            rule = rule.add_binding(read_binding(rpc, entry_message, input, output)?);
        }
    }

    Ok(Some(rule))
}

fn read_rule(
    rpc: &str,
    message: &DynamicMessage,
    input: &MessageDescriptor,
    output: &MessageDescriptor,
) -> Result<HttpRule, DescriptorError> {
    let (method, pattern) = read_pattern(rpc, message)?;
    let template = parse_template(rpc, &pattern)?;
    let request_body = read_body(message);
    let response_body = read_response_body(message);
    validate_body_field(rpc, &request_body, input)?;
    validate_response_body_field(rpc, &response_body, output)?;
    Ok(HttpRule::new(rpc, method, template)
        .request_body(request_body)
        .response_body(response_body))
}

fn read_binding(
    rpc: &str,
    message: &DynamicMessage,
    input: &MessageDescriptor,
    output: &MessageDescriptor,
) -> Result<Binding, DescriptorError> {
    let (method, pattern) = read_pattern(rpc, message)?;
    let template = parse_template(rpc, &pattern)?;
    let request_body = read_body(message);
    let response_body = read_response_body(message);
    validate_body_field(rpc, &request_body, input)?;
    validate_response_body_field(rpc, &response_body, output)?;
    Ok(Binding::new(method, template)
        .request_body(request_body)
        .response_body(response_body))
}

/// Verifies that a `body: "field"` names an actual field of the request
/// `message`, so a typo is caught at build time rather than silently
/// mis-transcoding at run time. `body: "*"` and no body need no field.
fn validate_body_field(rpc: &str, body: &RequestBody, message: &MessageDescriptor) -> Result<(), DescriptorError> {
    if let RequestBody::Field(field) = body {
        require_field(rpc, field, message, "body")?;
    }
    Ok(())
}

/// Verifies that a `response_body: "field"` names an actual field of the
/// response `message`. An empty/absent `response_body` (the whole message)
/// needs no field.
fn validate_response_body_field(rpc: &str, body: &ResponseBody, message: &MessageDescriptor) -> Result<(), DescriptorError> {
    if let ResponseBody::Field(field) = body {
        require_field(rpc, field, message, "response_body")?;
    }
    Ok(())
}

/// Returns an [`DescriptorError`] unless `message` has a field named `field`
/// (matched by either its proto or JSON name). `kind` is the annotation key
/// (`body` / `response_body`) for the diagnostic.
fn require_field(rpc: &str, field: &str, message: &MessageDescriptor, kind: &str) -> Result<(), DescriptorError> {
    if message.get_field_by_name(field).is_none() && message.get_field_by_json_name(field).is_none() {
        return Err(DescriptorError::unknown_field(rpc, kind, field, message.full_name()));
    }
    Ok(())
}

fn parse_template(rpc: &str, pattern: &str) -> Result<PathTemplate, DescriptorError> {
    PathTemplate::parse(pattern, Grammar::default()).map_err(|source| DescriptorError::template(rpc, pattern, source))
}

fn read_pattern(rpc: &str, message: &DynamicMessage) -> Result<(HttpMethod, String), DescriptorError> {
    for field in ["get", "put", "post", "delete", "patch"] {
        if message.has_field_by_name(field) {
            let value = message.get_field_by_name(field);
            let pattern = value
                .as_deref()
                .and_then(Value::as_str)
                .ok_or_else(|| DescriptorError::malformed(rpc, "pattern is not a string"))?
                .to_owned();
            let method = match field {
                "get" => HttpMethod::Get,
                "put" => HttpMethod::Put,
                "post" => HttpMethod::Post,
                "delete" => HttpMethod::Delete,
                _ => HttpMethod::Patch,
            };
            return Ok((method, pattern));
        }
    }

    if message.has_field_by_name("custom") {
        let value = message.get_field_by_name("custom");
        let custom = value
            .as_deref()
            .and_then(Value::as_message)
            .ok_or_else(|| DescriptorError::malformed(rpc, "custom pattern is not a message"))?;
        let kind = field_str(custom, "kind")
            .filter(|k| !k.is_empty())
            .ok_or_else(|| DescriptorError::malformed(rpc, "custom pattern has no kind"))?;
        let path = field_str(custom, "path")
            .filter(|p| !p.is_empty())
            .ok_or_else(|| DescriptorError::malformed(rpc, "custom pattern has no path"))?;
        return Ok((HttpMethod::Custom(kind.to_owned()), path.to_owned()));
    }

    Err(DescriptorError::no_pattern(rpc))
}

fn read_body(message: &DynamicMessage) -> RequestBody {
    match field_str(message, "body") {
        Some("") | None => RequestBody::None,
        Some("*") => RequestBody::Whole,
        Some(field) => RequestBody::Field(field.to_owned()),
    }
}

fn read_response_body(message: &DynamicMessage) -> ResponseBody {
    match field_str(message, "response_body") {
        Some("") | None => ResponseBody::Whole,
        Some(field) => ResponseBody::Field(field.to_owned()),
    }
}

/// Reads a string field's value, returning `None` for absent or non-string
/// fields. The borrow is copied out, so the returned `&str` is owned by `_`.
fn field_str<'m>(message: &'m DynamicMessage, field: &str) -> Option<&'m str> {
    // `get_field_by_name` yields `Some(Cow::Borrowed(..))` for a field stored on
    // the message and `Cow::Owned(default)` for an unset field; only
    // explicitly-present string values are reported.
    match message.get_field_by_name(field) {
        Some(std::borrow::Cow::Borrowed(value)) => value.as_str(),
        _ => None,
    }
}

/// Walks the dotted `field_path` from the `request` message to its leaf field
/// and, if that leaf is a proto `enum`, resolves its generated Rust type via
/// [`relative_type_path`]; otherwise returns `None`.
///
/// Non-leaf segments must be message fields (a path variable cannot descend
/// through a scalar); the walk stops — yielding `None` — if a segment is missing
/// or a non-leaf segment is not a message, which the descriptor validation and
/// template parsing already guard against.
fn enum_field_rust_type(
    request: &MessageDescriptor,
    field_path: &[String],
    package: &str,
    extern_paths: &[(String, String)],
) -> Option<String> {
    let mut current = request.clone();
    for (index, segment) in field_path.iter().enumerate() {
        let field = current.get_field_by_name(segment)?;
        if index + 1 == field_path.len() {
            return match field.kind() {
                Kind::Enum(descriptor) => Some(relative_type_path(descriptor.full_name(), package, extern_paths)),
                _ => None,
            };
        }
        current = match field.kind() {
            Kind::Message(message) => message,
            _ => return None,
        };
    }
    None
}

/// Resolves a message's fully-qualified proto name to a Rust type path.
///
/// `extern_paths` (proto path → Rust path) is consulted first, longest proto
/// prefix winning, so well-known types or types generated in another crate can
/// be redirected. Otherwise the type is resolved to a Rust path relative to
/// `package` (the service's package), matching `prost`/`pbjson` layout: a type
/// in the service's own package resolves to its simple name; a type in another
/// package is reached with one `super::` per remaining segment of `package`,
/// followed by the intervening package modules (snake-cased) and the type name.
fn relative_type_path(full_name: &str, package: &str, extern_paths: &[(String, String)]) -> String {
    if let Some(mapped) = resolve_extern(full_name, extern_paths) {
        return mapped;
    }

    let type_segs: Vec<&str> = full_name.split('.').filter(|s| !s.is_empty()).collect();
    let pkg_segs: Vec<&str> = package.split('.').filter(|s| !s.is_empty()).collect();
    let shared = pkg_segs.iter().zip(&type_segs).take_while(|(a, b)| a == b).count();

    let mut out = String::new();
    for _ in 0..(pkg_segs.len() - shared) {
        out.push_str("super::");
    }

    let rest = &type_segs[shared..];
    for (idx, seg) in rest.iter().enumerate() {
        if idx + 1 < rest.len() {
            out.push_str(&to_snake_case(seg));
            out.push_str("::");
        } else {
            out.push_str(seg);
        }
    }
    out
}

/// Applies the longest matching `extern_paths` override to `full_name`.
///
/// An exact match yields the mapped Rust path directly; a package-prefix match
/// appends the remaining proto segments (module segments snake-cased, the final
/// type name kept) to the mapped Rust path.
fn resolve_extern(full_name: &str, extern_paths: &[(String, String)]) -> Option<String> {
    let type_segs: Vec<&str> = full_name.split('.').filter(|s| !s.is_empty()).collect();

    let mut best: Option<(usize, &str)> = None;
    for (proto_path, rust_path) in extern_paths {
        let proto_segs: Vec<&str> = proto_path.split('.').filter(|s| !s.is_empty()).collect();
        if proto_segs.len() <= type_segs.len()
            && type_segs[..proto_segs.len()] == proto_segs[..]
            && best.is_none_or(|(len, _)| proto_segs.len() > len)
        {
            best = Some((proto_segs.len(), rust_path));
        }
    }

    let (matched_len, rust_path) = best?;
    // Root a bare extern path (e.g. `prost_types::Empty` -> `::prost_types::Empty`)
    // so the transcoder, emitted at an outer scope, can tell an extern override
    // apart from a package-relative type and not module-qualify it.
    let mut out = root_path(rust_path);
    let rest = &type_segs[matched_len..];
    for (idx, seg) in rest.iter().enumerate() {
        out.push_str("::");
        if idx + 1 < rest.len() {
            out.push_str(&to_snake_case(seg));
        } else {
            out.push_str(seg);
        }
    }
    Some(out)
}

/// Prefixes `::` to a Rust path unless it is already rooted (`::`, `crate`,
/// `super`, `self`, or `$crate`), so a bare crate-name path becomes absolute.
fn root_path(path: &str) -> String {
    let trimmed = path.trim_start();
    let rooted = trimmed.starts_with("::")
        || trimmed == "crate"
        || trimmed.starts_with("crate::")
        || trimmed == "super"
        || trimmed.starts_with("super::")
        || trimmed == "self"
        || trimmed.starts_with("self::")
        || trimmed.starts_with("$crate");
    if rooted { path.to_owned() } else { format!("::{path}") }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::{env, fs, process};

    use super::*;
    use crate::build::Generator;
    use crate::build::generator::CodegenOptions;

    static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn root_path_prefixes_only_unrooted_paths() {
        // A bare crate-name path is made absolute.
        assert_eq!(root_path("shelf::Book"), "::shelf::Book");
        assert_eq!(root_path("google::protobuf::Empty"), "::google::protobuf::Empty");
        // Each already-rooted form is passed through unchanged — one case per
        // branch of the `rooted` disjunction so no single condition can be
        // dropped or negated without a failure.
        assert_eq!(root_path("::already::Rooted"), "::already::Rooted");
        assert_eq!(root_path("crate"), "crate");
        assert_eq!(root_path("crate::pb::Shelf"), "crate::pb::Shelf");
        assert_eq!(root_path("super"), "super");
        assert_eq!(root_path("super::Sibling"), "super::Sibling");
        assert_eq!(root_path("self"), "self");
        assert_eq!(root_path("self::Nested"), "self::Nested");
        assert_eq!(root_path("$crate::Macro"), "$crate::Macro");
        // A path merely *containing* a keyword mid-string is still unrooted.
        assert_eq!(root_path("my_crate::Type"), "::my_crate::Type");
        assert_eq!(root_path("superb::Type"), "::superb::Type");
    }

    fn scratch_dir(name: &str) -> PathBuf {
        let suffix = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("rest_over_grpc_codegen_tests")
            .join(format!("{name}-{}-{suffix}", process::id()));
        fs::create_dir_all(&dir).expect("scratch directory is created under target");
        dir
    }

    /// The directory holding the vendored `google/api` annotation protos used as
    /// test fixtures when a test proto imports the http annotations.
    fn annotations_include() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("proto")
    }

    fn compile_descriptor(name: &str, source: &str, include_annotations: bool) -> Vec<u8> {
        let root = scratch_dir(name);
        let proto_name = format!("{name}.proto");
        fs::write(root.join(&proto_name), source).expect("test proto is written under target");

        let mut includes = vec![root];
        if include_annotations {
            includes.push(annotations_include());
        }
        let include_refs: Vec<_> = includes.iter().map(PathBuf::as_path).collect();

        let mut compiler = protox::Compiler::new(include_refs).expect("protox compiler initializes");
        compiler.include_imports(true);
        // Retain leading comments so tests can exercise proto-comment docs.
        compiler.include_source_info(true);
        compiler.open_file(&proto_name).expect("test proto compiles");
        compiler.encode_file_descriptor_set()
    }

    fn opts() -> DescriptorOptions {
        DescriptorOptions::new()
    }

    /// Compiles several `(filename, source)` proto files (each importing the http
    /// annotations) into one `FileDescriptorSet`, retaining source info.
    fn compile_descriptor_files(name: &str, files: &[(&str, &str)]) -> Vec<u8> {
        let root = scratch_dir(name);
        for (file_name, source) in files {
            fs::write(root.join(file_name), source).expect("test proto is written under target");
        }
        let annotations = annotations_include();
        let includes = [root.as_path(), annotations.as_path()];
        let mut compiler = protox::Compiler::new(includes).expect("protox compiler initializes");
        compiler.include_imports(true);
        compiler.include_source_info(true);
        for (file_name, _) in files {
            compiler.open_file(file_name).expect("test proto compiles");
        }
        compiler.encode_file_descriptor_set()
    }

    /// Concatenates the generated code for the definitions decoded from `bytes`,
    /// rendered with the default code-generation options.
    fn generated(bytes: &[u8], options: &DescriptorOptions) -> String {
        render(bytes, options, Generator::new())
    }

    /// Concatenates the generated code (traits, any tonic bridges, and the
    /// top-level transcoder) for the definitions decoded from `bytes`, rendered
    /// by the given generator.
    fn render(bytes: &[u8], options: &DescriptorOptions, mut generator: Generator) -> String {
        generator.add_all(definitions_from_descriptor(bytes, options).expect("definitions decode"));
        let (transcoder, outputs) = generator.generate();
        let mut code: String = outputs
            .iter()
            .map(|service| {
                let mut code = service.r#trait().to_string();
                if let Some(bridge) = service.tonic_bridge() {
                    code.push('\n');
                    code.push_str(&bridge.to_string());
                }
                code
            })
            .collect::<Vec<_>>()
            .join("\n");
        code.push('\n');
        code.push_str(&transcoder.to_string());
        code
    }

    fn valid_descriptor() -> Vec<u8> {
        compile_descriptor(
            "valid_annotations",
            r#"
                syntax = "proto3";
                package library;

                import "google/api/annotations.proto";

                message Request {
                  string name = 1;
                  string item = 2;
                  string patch = 3;
                }

                message Response {
                  string item = 1;
                }

                service Library {
                  rpc Get(Request) returns (Response) {
                    option (google.api.http) = {
                      get: "/v1/items/{name}"
                      response_body: "item"
                      additional_bindings {
                        get: "/v1/alternate/{name=items/*}"
                      }
                    };
                  }

                  rpc Post(Request) returns (Response) {
                    option (google.api.http) = {
                      post: "/v1/items"
                      body: "*"
                    };
                  }

                  rpc Put(Request) returns (Response) {
                    option (google.api.http) = {
                      put: "/v1/items/{name}"
                      body: "item"
                    };
                  }

                  rpc Delete(Request) returns (Response) {
                    option (google.api.http) = {
                      delete: "/v1/items/{name}"
                    };
                  }

                  rpc Patch(Request) returns (Response) {
                    option (google.api.http) = {
                      patch: "/v1/items/{name}"
                      body: "patch"
                    };
                  }

                  rpc Custom(Request) returns (Response) {
                    option (google.api.http) = {
                      custom {
                        kind: "HEAD"
                        path: "/v1/items/{name}:head"
                      }
                    };
                  }

                  rpc NoAnnotation(Request) returns (Response);
                }
            "#,
            true,
        )
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn unknown_body_field_is_rejected() {
        // `body: "missing"` names a field the request message does not have.
        let descriptor = compile_descriptor(
            "unknown_body_field",
            r#"
                syntax = "proto3";
                package badbody;
                import "google/api/annotations.proto";
                message Req { string name = 1; }
                message Resp { string item = 1; }
                service S {
                  rpc Put(Req) returns (Resp) {
                    option (google.api.http) = { put: "/v1/x/{name}" body: "missing" };
                  }
                }
            "#,
            true,
        );
        let err = definitions_from_descriptor(&descriptor, &DescriptorOptions::new()).expect_err("unknown body field is rejected");
        let message = err.to_string();
        assert!(message.contains("`body`"), "{message}");
        assert!(message.contains("missing"), "{message}");
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn unknown_response_body_field_is_rejected() {
        // `response_body: "nope"` names a field the response message does not have.
        let descriptor = compile_descriptor(
            "unknown_response_body_field",
            r#"
                syntax = "proto3";
                package badresp;
                import "google/api/annotations.proto";
                message Req { string name = 1; }
                message Resp { string item = 1; }
                service S {
                  rpc Get(Req) returns (Resp) {
                    option (google.api.http) = { get: "/v1/x/{name}" response_body: "nope" };
                  }
                }
            "#,
            true,
        );
        let err = definitions_from_descriptor(&descriptor, &DescriptorOptions::new()).expect_err("unknown response_body field is rejected");
        let message = err.to_string();
        assert!(message.contains("`response_body`"), "{message}");
        assert!(message.contains("nope"), "{message}");
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn snake_case_body_field_is_accepted() {
        // `body`/`response_body` may name a field by its proto (snake_case) name
        // even when its JSON name differs; validation must accept it (this guards
        // the `&&` combining the proto-name and JSON-name lookups).
        let descriptor = compile_descriptor(
            "snake_case_body",
            r#"
                syntax = "proto3";
                package snakebody;
                import "google/api/annotations.proto";
                message Req { string page_size = 1; }
                message Resp { string next_page_token = 1; }
                service S {
                  rpc Update(Req) returns (Resp) {
                    option (google.api.http) = { post: "/v1/x" body: "page_size" response_body: "next_page_token" };
                  }
                }
            "#,
            true,
        );
        let definitions =
            definitions_from_descriptor(&descriptor, &DescriptorOptions::new()).expect("snake_case body/response_body fields are accepted");
        assert_eq!(definitions.len(), 1);
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn transcoder_resolves_cross_package_request_types() {
        // A service in package `a` whose RPC takes a message from package `b`.
        // The trait (included inside `mod a`) uses the package-relative
        // `super::b::Req`, but the top-level transcoder (emitted at the common
        // root) must reference the absolute `b::Req` — not a `super::` path, which
        // would not resolve from the transcoder's scope.
        let files = &[
            (
                "b.proto",
                r#"
                    syntax = "proto3";
                    package b;
                    message Req { string id = 1; }
                    message Resp { string v = 1; }
                "#,
            ),
            (
                "a.proto",
                r#"
                    syntax = "proto3";
                    package a;
                    import "google/api/annotations.proto";
                    import "b.proto";
                    service ASvc {
                      rpc Get(b.Req) returns (b.Resp) { option (google.api.http) = { get: "/v1/x/{id}" }; }
                    }
                "#,
            ),
        ];
        let descriptor = compile_descriptor_files("cross_pkg", files);
        let defs = definitions_from_descriptor(&descriptor, &DescriptorOptions::new()).expect("valid annotations");
        let mut generator = Generator::new();
        generator.add_all(defs);
        let transcoder = generator.generate().0.to_string();

        // The transcoder decodes into the absolute `b::Req` and bounds the handler
        // by the absolute `a::ASvc`, with no `super::`-relative leftover.
        assert!(transcoder.contains("b :: Req"), "{transcoder}");
        assert!(transcoder.contains("a :: ASvc"), "{transcoder}");
        assert!(!transcoder.contains("super"), "{transcoder}");
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn proto_leading_comments_are_correct_across_multiple_files() {
        // Across two service-bearing files, each service's method keeps its own
        // leading comment (the doc lookup path is file-relative).
        let one = (
            "one.proto",
            r#"
                syntax = "proto3";
                package one;
                import "google/api/annotations.proto";
                message R { string id = 1; }
                service AService {
                  // Comment for AService.Get.
                  rpc Get(R) returns (R) { option (google.api.http) = { get: "/one/{id}" }; }
                }
            "#,
        );
        let two = (
            "two.proto",
            r#"
                syntax = "proto3";
                package two;
                import "google/api/annotations.proto";
                message R { string id = 1; }
                service BService {
                  // Comment for BService.Get.
                  rpc Get(R) returns (R) { option (google.api.http) = { get: "/two/{id}" }; }
                }
            "#,
        );
        let descriptor = compile_descriptor_files("multi_file_docs", &[one, two]);
        let definitions = definitions_from_descriptor(&descriptor, &DescriptorOptions::new()).expect("valid annotations");

        let doc_of = |trait_name: &str| {
            definitions
                .iter()
                .find(|d| d.trait_name() == trait_name)
                .expect("service present")
                .generate(CodegenOptions::default())
                .to_string()
        };

        // Each service's method keeps its own comment — not dropped, not swapped.
        let a = doc_of("AService");
        assert!(a.contains("Comment for AService.Get."), "{a}");
        assert!(!a.contains("Comment for BService.Get."), "{a}");
        let b = doc_of("BService");
        assert!(b.contains("Comment for BService.Get."), "{b}");
        assert!(!b.contains("Comment for AService.Get."), "{b}");
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn proto_leading_comments_become_method_docs() {
        // A method's proto leading comment is applied verbatim (all lines) to the
        // generated trait method; a comment-less method gets no doc comment.
        let descriptor = compile_descriptor(
            "method_docs",
            r#"
                syntax = "proto3";
                package docs;
                import "google/api/annotations.proto";
                message Req { string name = 1; }
                message Resp { string value = 1; }
                service S {
                  // Fetches a single item by name.
                  // A second line of documentation.
                  rpc Get(Req) returns (Resp) {
                    option (google.api.http) = { get: "/v1/items/{name}" };
                  }
                  rpc List(Req) returns (Resp) {
                    option (google.api.http) = { get: "/v1/items" };
                  }
                }
            "#,
            true,
        );
        let definitions = definitions_from_descriptor(&descriptor, &DescriptorOptions::new()).expect("valid annotations");
        let rendered = definitions[0].trait_code().to_string();

        // The documented RPC carries its proto comment (both lines).
        assert!(rendered.contains("Fetches a single item by name."), "{rendered}");
        assert!(rendered.contains("A second line of documentation."), "{rendered}");
        // The comment-less RPC carries no doc comment: only the documented RPC's
        // two `#[doc]` lines are emitted (the service itself is uncommented).
        assert_eq!(rendered.matches("[doc").count(), 2, "{rendered}");
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn proto_service_leading_comment_documents_the_trait() {
        // A service's leading comment is applied to the generated trait; a
        // comment-less service yields a trait with no doc comment.
        let descriptor = compile_descriptor(
            "service_docs",
            r#"
                syntax = "proto3";
                package docs;
                import "google/api/annotations.proto";
                message Req { string name = 1; }
                message Resp { string value = 1; }
                // Manages the shelves in the library.
                service Library {
                  rpc Get(Req) returns (Resp) { option (google.api.http) = { get: "/v1/items/{name}" }; }
                }
                service Undocumented {
                  rpc Get(Req) returns (Resp) { option (google.api.http) = { get: "/v2/items/{name}" }; }
                }
            "#,
            true,
        );
        let definitions = definitions_from_descriptor(&descriptor, &DescriptorOptions::new()).expect("valid annotations");
        let library = definitions.iter().find(|d| d.trait_name() == "Library").expect("Library");
        let undocumented = definitions.iter().find(|d| d.trait_name() == "Undocumented").expect("Undocumented");

        // The service's leading comment documents the trait; its comment-less RPC
        // adds no further doc, so exactly one `#[doc]` is emitted.
        let library_code = library.trait_code().to_string();
        assert!(library_code.contains("Manages the shelves in the library."), "{library_code}");
        assert_eq!(library_code.matches("[doc").count(), 1, "{library_code}");

        // A comment-less service yields a trait (and method) with no doc comment.
        let undocumented_code = undocumented.trait_code().to_string();
        assert!(!undocumented_code.contains("[doc"), "{undocumented_code}");
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn maps_type_paths() {
        assert_eq!(relative_type_path("library.GetShelfRequest", "library", &[]), "GetShelfRequest");
        assert_eq!(relative_type_path("a.b.c.Message", "a.b.c", &[]), "Message");
        assert_eq!(relative_type_path("NoPackage", "", &[]), "NoPackage");
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn decode_error_is_reported() {
        let err = definitions_from_descriptor(b"not a descriptor set", &opts()).expect_err("invalid descriptor bytes");
        assert!(err.to_string().contains("decode"));
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn reads_annotated_unary_services() {
        let services = definitions_from_descriptor(&valid_descriptor(), &opts()).expect("annotations are read");

        assert_eq!(services.len(), 1);
        assert_eq!(services[0].module_name(), "library");
        let code = services[0].generate(CodegenOptions::default()).to_string();
        assert!(code.contains("pub trait Library"));
        assert!(code.contains("get"));
        assert!(code.contains("post"));
        assert!(code.contains("put"));
        assert!(code.contains("delete"));
        assert!(code.contains("patch"));
        assert!(code.contains("custom"));
        assert!(code.contains("HEAD"));
        assert!(code.contains("Request"));
        assert!(code.contains("Response"));
        assert!(code.contains("RequestBodyKind :: Whole"));
        assert!(code.contains("RequestBodyKind :: Field"));
        assert!(code.contains("ResponseBodyKind :: Field"));
        assert!(!code.contains("no_annotation"));
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn skips_services_when_http_extension_is_absent() {
        let descriptor = compile_descriptor(
            "no_http_extension",
            r#"
                syntax = "proto3";
                package library;

                message Request {}
                message Response {}

                service Library {
                  rpc Unannotated(Request) returns (Response);
                }
            "#,
            false,
        );

        let services = definitions_from_descriptor(&descriptor, &opts()).expect("descriptor is valid");

        assert!(services.is_empty());
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn rejects_streaming_annotated_methods() {
        let descriptor = compile_descriptor(
            "streaming",
            r#"
                syntax = "proto3";
                package library;

                import "google/api/annotations.proto";

                message Request {}
                message Response {}

                service Library {
                  rpc Stream(stream Request) returns (Response) {
                    option (google.api.http) = {
                      get: "/v1/stream"
                    };
                  }
                }
            "#,
            true,
        );

        let err = definitions_from_descriptor(&descriptor, &opts()).expect_err("streaming is rejected");

        assert!(err.to_string().contains("streaming"));
        assert!(std::error::Error::source(&err).is_none());
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn reports_rules_without_patterns() {
        let descriptor = compile_descriptor(
            "no_pattern",
            r#"
                syntax = "proto3";
                package library;

                import "google/api/annotations.proto";

                message Request {}
                message Response {}

                service Library {
                  rpc MissingPattern(Request) returns (Response) {
                    option (google.api.http) = {
                      body: "*"
                    };
                  }
                }
            "#,
            true,
        );

        let err = definitions_from_descriptor(&descriptor, &opts()).expect_err("pattern is required");

        assert!(err.to_string().contains("no URL pattern"));
        assert!(std::error::Error::source(&err).is_none());
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn surfaces_invalid_path_template() {
        let descriptor = compile_descriptor(
            "invalid_template",
            r#"
                syntax = "proto3";
                package library;

                import "google/api/annotations.proto";

                message Request {}
                message Response {}

                service Library {
                  rpc BadTemplate(Request) returns (Response) {
                    option (google.api.http) = {
                      get: "no-leading-slash"
                    };
                  }
                }
            "#,
            true,
        );

        let err = definitions_from_descriptor(&descriptor, &opts()).expect_err("bad template is rejected");

        assert!(err.to_string().contains("invalid path template"));
        assert!(std::error::Error::source(&err).is_some());
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn malformed_extension_values_are_reported() {
        let descriptor = compile_descriptor(
            "malformed_extension",
            r#"
                syntax = "proto2";
                package malformed;

                import "google/protobuf/descriptor.proto";

                extend google.protobuf.MethodOptions {
                  optional string scalar_http = 50001;
                }
            "#,
            false,
        );
        let pool = DescriptorPool::decode(descriptor.as_slice()).expect("descriptor pool decodes");
        let options_desc = pool
            .get_message_by_name("google.protobuf.MethodOptions")
            .expect("method options descriptor is present");
        let scalar_ext = pool
            .get_extension_by_name("malformed.scalar_http")
            .expect("scalar extension is present");
        let mut options = DynamicMessage::new(options_desc);
        options.set_extension(&scalar_ext, Value::String("not a message".into()));

        let msg = pool
            .get_message_by_name("google.protobuf.MethodOptions")
            .expect("method options descriptor is present");
        let err = read_http_rule("Malformed", &options, Some(&scalar_ext), &msg, &msg).expect_err("non-message extension is malformed");

        assert!(err.to_string().contains("malformed http annotation"));
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn extensions_without_additional_bindings_field_are_supported() {
        let descriptor = compile_descriptor(
            "fake_http_extension",
            r#"
                syntax = "proto2";
                package fake;

                import "google/protobuf/descriptor.proto";

                message FakeHttp {
                  optional string get = 1;
                }

                extend google.protobuf.MethodOptions {
                  optional FakeHttp fake_http = 50002;
                }
            "#,
            false,
        );
        let pool = DescriptorPool::decode(descriptor.as_slice()).expect("descriptor pool decodes");
        let options_desc = pool
            .get_message_by_name("google.protobuf.MethodOptions")
            .expect("method options descriptor is present");
        let fake_desc = pool.get_message_by_name("fake.FakeHttp").expect("fake http descriptor is present");
        let fake_ext = pool.get_extension_by_name("fake.fake_http").expect("fake extension is present");
        let mut fake_http = DynamicMessage::new(fake_desc.clone());
        fake_http.set_field_by_name("get", Value::String("/v1/fake".into()));
        let mut options = DynamicMessage::new(options_desc);
        options.set_extension(&fake_ext, Value::Message(fake_http));

        let rule = read_http_rule("Fake", &options, Some(&fake_ext), &fake_desc, &fake_desc)
            .expect("fake annotation is valid")
            .expect("fake annotation exists");
        let routes = rule.lower();

        assert_eq!(routes.len(), 1);
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn malformed_http_message_fields_are_reported() {
        let descriptor = compile_descriptor(
            "malformed_http_fields",
            r#"
                syntax = "proto2";
                package malformed_fields;

                import "google/protobuf/descriptor.proto";

                message BadGet {
                  optional int32 get = 1;
                }

                message BadCustom {
                  optional string custom = 1;
                }

                message BadAdditional {
                  optional string get = 1;
                  repeated string additional_bindings = 2;
                }

                extend google.protobuf.MethodOptions {
                  optional BadGet bad_get = 50003;
                  optional BadCustom bad_custom = 50004;
                  optional BadAdditional bad_additional = 50005;
                }
            "#,
            false,
        );
        let pool = DescriptorPool::decode(descriptor.as_slice()).expect("descriptor pool decodes");
        let options_desc = pool
            .get_message_by_name("google.protobuf.MethodOptions")
            .expect("method options descriptor is present");
        // A stand-in message for the input/output arguments: these cases fail on
        // the pattern before body/response_body validation is reached.
        let msg = options_desc.clone();

        let bad_get_ext = pool
            .get_extension_by_name("malformed_fields.bad_get")
            .expect("bad_get extension is present");
        let mut bad_get = DynamicMessage::new(
            pool.get_message_by_name("malformed_fields.BadGet")
                .expect("BadGet descriptor is present"),
        );
        bad_get.set_field_by_name("get", Value::I32(1));
        let mut bad_get_options = DynamicMessage::new(options_desc.clone());
        bad_get_options.set_extension(&bad_get_ext, Value::Message(bad_get));
        let err = read_http_rule("BadGet", &bad_get_options, Some(&bad_get_ext), &msg, &msg).expect_err("pattern type is rejected");
        assert!(err.to_string().contains("pattern is not a string"));

        let bad_custom_ext = pool
            .get_extension_by_name("malformed_fields.bad_custom")
            .expect("bad_custom extension is present");
        let mut bad_custom = DynamicMessage::new(
            pool.get_message_by_name("malformed_fields.BadCustom")
                .expect("BadCustom descriptor is present"),
        );
        bad_custom.set_field_by_name("custom", Value::String("not a message".into()));
        let mut bad_custom_options = DynamicMessage::new(options_desc.clone());
        bad_custom_options.set_extension(&bad_custom_ext, Value::Message(bad_custom));
        let err = read_http_rule("BadCustom", &bad_custom_options, Some(&bad_custom_ext), &msg, &msg)
            .expect_err("custom pattern type is rejected");
        assert!(err.to_string().contains("custom pattern is not a message"));

        let bad_additional_ext = pool
            .get_extension_by_name("malformed_fields.bad_additional")
            .expect("bad_additional extension is present");
        let mut bad_additional = DynamicMessage::new(
            pool.get_message_by_name("malformed_fields.BadAdditional")
                .expect("BadAdditional descriptor is present"),
        );
        bad_additional.set_field_by_name("get", Value::String("/v1/bad".into()));
        bad_additional.set_field_by_name("additional_bindings", Value::List(vec![Value::String("bad".into())]));
        let mut bad_additional_options = DynamicMessage::new(options_desc);
        bad_additional_options.set_extension(&bad_additional_ext, Value::Message(bad_additional));
        let err = read_http_rule("BadAdditional", &bad_additional_options, Some(&bad_additional_ext), &msg, &msg)
            .expect_err("additional binding type is rejected");
        assert!(err.to_string().contains("additional_binding is not a message"));
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn field_helpers_distinguish_absent_empty_and_non_string_values() {
        let descriptor = valid_descriptor();
        let pool = DescriptorPool::decode(descriptor.as_slice()).expect("descriptor pool decodes");
        let http_ext = pool.get_extension_by_name("google.api.http").expect("http extension is present");
        let service = pool.get_service_by_name("library.Library").expect("service is present");

        let get = service
            .methods()
            .find(|method| method.name() == "Get")
            .expect("GET method is present");
        let get_options = get.options();
        let get_rule = read_http_rule(get.name(), &get_options, Some(&http_ext), &get.input(), &get.output())
            .expect("GET annotation is valid")
            .expect("GET annotation exists");
        let get_routes = get_rule.lower();
        assert_eq!(get_routes.len(), 2);
        assert_eq!(get_routes[0].body(), &RequestBody::None);
        assert_eq!(get_routes[0].response_body(), &ResponseBody::Field("item".into()));

        let post = service
            .methods()
            .find(|method| method.name() == "Post")
            .expect("POST method is present");
        let post_options = post.options();
        let post_rule = read_http_rule(post.name(), &post_options, Some(&http_ext), &post.input(), &post.output())
            .expect("POST annotation is valid")
            .expect("POST annotation exists");
        let post_routes = post_rule.lower();
        assert_eq!(post_routes[0].body(), &RequestBody::Whole);
        assert_eq!(post_routes[0].response_body(), &ResponseBody::Whole);

        let custom = service
            .methods()
            .find(|method| method.name() == "Custom")
            .expect("custom method is present");
        let custom_options = custom.options();
        let custom_value = custom_options.get_extension(&http_ext);
        let custom_message = custom_value.as_message().expect("custom annotation is a message");
        assert_eq!(field_str(custom_message, "custom"), None);
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn from_fds_produces_service_code() {
        let descriptor = compile_descriptor(
            "oneshot",
            r#"
                syntax = "proto3";
                package oneshot;
                import "google/api/annotations.proto";
                message GetReq { string name = 1; }
                message GetResp { string value = 1; }
                service OneShot {
                    rpc Get(GetReq) returns (GetResp) {
                        option (google.api.http) = { get: "/v1/{name}" };
                    }
                }
            "#,
            true,
        );

        let code = generated(&descriptor, &opts());

        assert!(code.contains("pub trait OneShot"));
        assert!(code.contains("fn try_transcode"));
        assert!(code.contains("GetReq"));

        let definitions = definitions_from_descriptor(&descriptor, &opts()).expect("decodes");
        assert_eq!(definitions.len(), 1);
        assert_eq!(definitions[0].module_name(), "oneshot");
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn from_fds_classifies_an_enum_path_variable() {
        // An `enum` field bound as a path variable is classified so the generated
        // poke accepts the value by name (via `parse_path_enum_value`), not just
        // as a scalar.
        let descriptor = compile_descriptor(
            "enumpath",
            r#"
                syntax = "proto3";
                package enumpath;
                import "google/api/annotations.proto";
                enum State { STATE_UNSPECIFIED = 0; ACTIVE = 1; }
                message Req { State state = 1; }
                message Resp { string value = 1; }
                service Svc {
                    rpc Get(Req) returns (Resp) {
                        option (google.api.http) = { get: "/v1/state/{state}" };
                    }
                }
            "#,
            true,
        );

        let code = generated(&descriptor, &opts());
        assert!(code.contains("parse_path_enum_value"), "{code}");
        assert!(
            code.contains("State :: from_str_name") || code.contains("State::from_str_name"),
            "{code}"
        );
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn from_fds_classifies_a_nested_enum_path_variable() {
        // A dotted path variable whose leaf is an `enum` reached *through* a nested
        // message (`{filters.genre}`) must still be classified as an enum, so the
        // walk has to descend into the intermediate message field before reaching
        // the enum leaf.
        let descriptor = compile_descriptor(
            "nestedenum",
            r#"
                syntax = "proto3";
                package nestedenum;
                import "google/api/annotations.proto";
                enum Genre { GENRE_UNSPECIFIED = 0; SCIENCE = 1; }
                message Filters { Genre genre = 1; }
                message Req { Filters filters = 1; }
                message Resp { string value = 1; }
                service Svc {
                    rpc Get(Req) returns (Resp) {
                        option (google.api.http) = { get: "/v1/books/{filters.genre}" };
                    }
                }
            "#,
            true,
        );

        let code = generated(&descriptor, &opts());
        // Classified as an enum (parsed by name/number), not a bare scalar, which
        // is only possible if the walk descended through the `filters` message to
        // reach the `genre` enum leaf.
        assert!(code.contains("parse_path_enum_value"), "{code}");
        assert!(
            code.contains("Genre :: from_str_name") || code.contains("Genre::from_str_name"),
            "{code}"
        );
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn enum_classification_skips_a_path_variable_that_descends_through_a_scalar() {
        // A dotted path variable whose first segment is a scalar (not a message)
        // cannot be an enum field, so classification bails without error.
        let descriptor = compile_descriptor(
            "scalarpath",
            r#"
                syntax = "proto3";
                package scalarpath;
                import "google/api/annotations.proto";
                message Req { string a = 1; }
                message Resp { string value = 1; }
                service Svc {
                    rpc Get(Req) returns (Resp) {
                        option (google.api.http) = { get: "/v1/{a.b}" };
                    }
                }
            "#,
            true,
        );

        let definitions = definitions_from_descriptor(&descriptor, &opts()).expect("decodes despite the scalar-intermediate path");
        assert_eq!(definitions.len(), 1);
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn enum_field_rust_type_is_none_for_an_empty_field_path() {
        // Defensive fall-through: an empty field path never occurs for a real
        // path variable (each has at least one segment), so the walk yields `None`.
        let descriptor = compile_descriptor(
            "emptypath",
            r#"
                syntax = "proto3";
                package emptypath;
                message Req { string a = 1; }
            "#,
            false,
        );
        let pool = prost_reflect::DescriptorPool::decode(descriptor.as_slice()).expect("descriptor pool decodes");
        let message = pool.get_message_by_name("emptypath.Req").expect("Req message exists");
        assert_eq!(enum_field_rust_type(&message, &[], "emptypath", &[]), None);
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn compile_fds_writes_module_and_transcoder_files() {
        let descriptor = compile_descriptor(
            "compilefds",
            r#"
                syntax = "proto3";
                package compilefds;
                import "google/api/annotations.proto";
                message GetReq { string name = 1; }
                message GetResp { string value = 1; }
                service OneShot {
                    rpc Get(GetReq) returns (GetResp) {
                        option (google.api.http) = { get: "/v1/{name}" };
                    }
                }
            "#,
            true,
        );
        let out = scratch_dir("compilefds-out");
        crate::build::compile_fds(&descriptor, &out).expect("descriptor compiles and writes");
        assert!(out.join("compilefds.rest.rs").exists(), "the service module is written");
        assert!(out.join("transcoder.rest.rs").exists(), "the transcoder is written");
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn compile_fds_propagates_a_descriptor_decode_error() {
        // Invalid descriptor bytes fail to decode, so `compile_fds` surfaces the
        // error (the `?` error path) rather than writing anything.
        let out = scratch_dir("compilefds-bad");
        crate::build::compile_fds(b"\xff\xff not a FileDescriptorSet", &out).expect_err("invalid descriptor bytes error");
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn leading_comment_ignores_a_blank_comment() {
        // A present-but-blank leading comment yields no doc (the blank branch),
        // while a real comment is retained.
        let descriptor = compile_descriptor(
            "blankdoc",
            "syntax = \"proto3\";\npackage blankdoc;\nimport \"google/api/annotations.proto\";\nmessage R { string name = 1; }\n//\u{20}\nservice Blank {\n  rpc Get(R) returns (R) { option (google.api.http) = { get: \"/v1/{name}\" }; }\n}\n",
            true,
        );
        // The service's blank comment must not become a doc string on the trait.
        let code = generated(&descriptor, &opts());
        assert!(code.contains("trait Blank"), "{code}");
    }

    #[cfg(feature = "build-openapi")]
    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn write_merges_openapi_specs_for_services_sharing_a_module() {
        // Two services in one proto package share a `{module}.openapi.json`, so
        // their per-service specs are merged (paths and schemas unioned).
        let descriptor = compile_descriptor(
            "multi",
            r#"
                syntax = "proto3";
                package multi;
                import "google/api/annotations.proto";
                message AReq { string a = 1; }
                message AResp { string va = 1; }
                message BReq { string b = 1; }
                message BResp { string vb = 1; }
                service Alpha {
                    rpc GetA(AReq) returns (AResp) {
                        option (google.api.http) = { get: "/v1/alpha/{a}" };
                    }
                }
                service Beta {
                    rpc GetB(BReq) returns (BResp) {
                        option (google.api.http) = { get: "/v1/beta/{b}" };
                    }
                }
            "#,
            true,
        );
        let definitions = ServiceDefinition::from_fds(&descriptor, &opts()).expect("decodes");
        let mut generator = Generator::builder()
            .emit_openapi_spec(Some(crate::build::OpenApiInfo::new("Multi", "v1")))
            .build();
        generator.add_all(definitions);
        let out = scratch_dir("multi-out");
        generator.write(&out).expect("writes merged output");

        let spec = fs::read_to_string(out.join("multi.openapi.json")).expect("merged openapi document exists");
        // Paths from both services are unioned into the one module document.
        assert!(spec.contains("/v1/alpha/{a}"), "alpha path merged: {spec}");
        assert!(spec.contains("/v1/beta/{b}"), "beta path merged: {spec}");
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn prefix_filters_services_by_package() {
        assert_eq!(
            definitions_from_descriptor(&valid_descriptor(), &opts().package(".library"))
                .expect("matches")
                .len(),
            1
        );
        assert!(
            definitions_from_descriptor(&valid_descriptor(), &opts().package(".other"))
                .expect("no match")
                .is_empty()
        );
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn relative_type_path_is_package_relative() {
        assert_eq!(relative_type_path("library.Shelf", "library", &[]), "Shelf");
        assert_eq!(relative_type_path("library.Outer.Inner", "library", &[]), "outer::Inner");
        assert_eq!(relative_type_path("other.Thing", "library", &[]), "super::other::Thing");
        assert_eq!(relative_type_path("a.b.Deep", "a.b", &[]), "Deep");
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn extern_path_overrides_type_resolution() {
        let externs = vec![
            (".google.protobuf.Empty".to_owned(), "::prost_types::Empty".to_owned()),
            (".google.protobuf".to_owned(), "::prost_types".to_owned()),
        ];
        // Exact match wins.
        assert_eq!(
            relative_type_path("google.protobuf.Empty", "library", &externs),
            "::prost_types::Empty"
        );
        // Longer prefix (package) applies to a sibling type.
        assert_eq!(
            relative_type_path("google.protobuf.Timestamp", "library", &externs),
            "::prost_types::Timestamp"
        );
        // No matching extern falls back to relative resolution.
        assert_eq!(relative_type_path("library.Shelf", "library", &externs), "Shelf");
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn extern_resolution_prefers_the_longest_matching_prefix() {
        // Two prefixes of the same type; the longer (more specific) one must win,
        // regardless of declaration order.
        let externs = vec![
            (".google".to_owned(), "::short".to_owned()),
            (".google.protobuf".to_owned(), "::long".to_owned()),
        ];
        assert_eq!(relative_type_path("google.protobuf.Empty", "library", &externs), "::long::Empty");

        // With equal-length (duplicate) prefixes, the first declared wins.
        let dup = vec![
            (".a.b".to_owned(), "::first".to_owned()),
            (".a.b".to_owned(), "::second".to_owned()),
        ];
        assert_eq!(relative_type_path("a.b.Thing", "library", &dup), "::first::Thing");
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn extern_prefix_snake_cases_intermediate_module_segments() {
        // A prefix match leaving multiple trailing segments snake-cases the
        // intermediate module segments and keeps the final type name verbatim.
        let externs = vec![(".root".to_owned(), "::root".to_owned())];
        assert_eq!(relative_type_path("root.MyMod.Thing", "library", &externs), "::root::my_mod::Thing");
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn every_http_method_is_emitted_with_its_verb() {
        // `valid_descriptor` annotates one RPC per method (plus a custom `HEAD`);
        // each must route under its own verb, not collapse to the `_` fallback.
        let code = generated(&valid_descriptor(), &opts());
        for method in ["GET", "PUT", "POST", "DELETE", "PATCH", "HEAD"] {
            assert!(
                code.contains(&format!("\"{method}\"")),
                "generated router is missing a `{method}` route"
            );
        }
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn server_streaming_generates_a_stream_method() {
        let descriptor = compile_descriptor(
            "server_streaming",
            r#"
                syntax = "proto3";
                package library;
                import "google/api/annotations.proto";
                message ListReq { string filter = 1; }
                message Shelf { string name = 1; }
                service Library {
                    rpc List(ListReq) returns (stream Shelf) {
                        option (google.api.http) = { get: "/v1/shelves" };
                    }
                }
            "#,
            true,
        );

        let code = generated(&descriptor, &opts());
        assert!(code.contains("rest_over_grpc :: handling :: ResponseStream"));
        assert!(code.contains("StreamingResponse :: encode"));
        assert!(code.contains("StreamEncoding :: from_accept"));
        // The service has a server-streaming RPC, so `try_transcode`
        // emits a streaming arm producing a `TranscodeResponse`.
        assert!(code.contains("fn try_transcode"));
        assert!(!code.contains("try_transcode_streaming"));
        assert!(code.contains("rest_over_grpc :: transcoding :: TranscodeResponse"));
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn send_bounds_emit_send_and_supertrait() {
        // `Send` bounds are always emitted so the output works on multi-threaded
        // executors.
        let code = render(&valid_descriptor(), &opts(), Generator::new());
        assert!(code.contains("Send + :: core :: marker :: Sync") || code.contains("Send + ::core::marker::Sync"));
        assert!(code.matches("marker :: Send").count() >= 2);
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn tonic_generator_option_emits_the_bridge() {
        // On by default: the bridge `impl` is emitted for the decoded service.
        let bridged = render(&valid_descriptor(), &opts(), Generator::new());
        assert!(bridged.replace(' ', "").contains("library_server::Library"));

        // Opting out drops the bridge `impl`.
        let plain = render(&valid_descriptor(), &opts(), Generator::builder().emit_tonic_bridge(false).build());
        assert!(!plain.replace(' ', "").contains("library_server::Library"));
    }
}
