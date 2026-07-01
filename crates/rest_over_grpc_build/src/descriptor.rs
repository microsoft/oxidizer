// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Reading `google.api.http` annotations from a compiled `FileDescriptorSet`.
//!
//! [`services_from_descriptor`] decodes a `FileDescriptorSet` (as produced by
//! `protox` / `protoc`), walks each service's methods, reads the
//! `google.api.http` method-options extension, and builds the [`Service`]
//! values that [`Service::generate`] turns into a router + trait + dispatcher.
//!
//! This removes the need to hand-write [`HttpRule`]s: the bindings come straight
//! from the proto annotations.
//!
//! # Type mapping
//!
//! Each RPC's request/response message is mapped to a Rust type path of the form
//! `{type_root}::{MessageName}` (the message's simple name appended to the
//! caller-provided `type_root`, e.g. `"crate::pb"`). This matches the common
//! layout where a single proto package is `prost`-generated and `include!`d into
//! one module. Multi-package layouts and nested messages are not yet mapped.

use core::fmt;
use std::backtrace::Backtrace;

use prost_reflect::{DescriptorPool, DynamicMessage, ExtensionDescriptor, Value};

use crate::body::Body;
use crate::http_method::HttpMethod;
use crate::http_rule::HttpRule;
use crate::response_body::ResponseBody;
use crate::rule_error::RuleError;
use crate::service::Service;
use crate::service_method::ServiceMethod;

/// Decodes a `FileDescriptorSet` and builds a [`Service`] per gRPC service that
/// has at least one `google.api.http`-annotated method.
///
/// `type_root` is the Rust module path under which the `prost`-generated message
/// types live (for example `"crate::pb"`).
///
/// # Errors
///
/// Returns a [`DescriptorError`] if the descriptor bytes cannot be decoded, if a
/// method's annotation is malformed, if an annotated method uses streaming
/// (which cannot be transcoded), or if a path template fails to parse.
///
/// # Examples
///
/// ```no_run
/// # fn main() {
/// # #[cfg(feature = "descriptor")] {
/// use rest_over_grpc_build::services_from_descriptor;
///
/// let descriptor_set = std::fs::read("target/file_descriptor_set.bin")
///     .expect("the build script wrote a FileDescriptorSet");
/// let services = services_from_descriptor(&descriptor_set, "crate::pb")
///     .expect("descriptor annotations are valid");
///
/// let tokens: Vec<_> = services.iter().map(|service| service.generate()).collect();
/// assert!(tokens.iter().all(|tokens| !tokens.to_string().is_empty()));
/// # }
/// # }
/// ```
pub fn services_from_descriptor(descriptor_set: &[u8], type_root: &str) -> Result<Vec<Service>, DescriptorError> {
    let pool = DescriptorPool::decode(descriptor_set).map_err(|e| DescriptorError::decode(&e.to_string()))?;
    let http_ext = pool.get_extension_by_name("google.api.http");

    let mut services = Vec::new();
    for service in pool.services() {
        let mut methods = Vec::new();
        for method in service.methods() {
            let Some(rule) = read_http_rule(method.name(), &method.options(), http_ext.as_ref())? else {
                continue;
            };

            if method.is_client_streaming() || method.is_server_streaming() {
                return Err(DescriptorError::streaming(method.full_name()));
            }

            let routes = rule.lower().map_err(DescriptorError::rule)?;
            let request_type = rust_type_path(method.input().full_name(), type_root);
            let response_type = rust_type_path(method.output().full_name(), type_root);
            methods.push(ServiceMethod::new(method.name(), (request_type, response_type), routes));
        }

        if !methods.is_empty() {
            services.push(Service::new(service.name(), methods));
        }
    }

    Ok(services)
}

fn read_http_rule(
    rpc: &str,
    options: &DynamicMessage,
    http_ext: Option<&ExtensionDescriptor>,
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

    let mut rule = read_basic_rule(rpc, message)?;

    let additional = message.get_field_by_name("additional_bindings");
    if let Some(list) = additional.as_deref().and_then(Value::as_list) {
        for entry in list {
            let entry_message = entry
                .as_message()
                .ok_or_else(|| DescriptorError::malformed(rpc, "additional_binding is not a message"))?;
            rule = rule.with_additional_binding(read_basic_rule(rpc, entry_message)?);
        }
    }

    Ok(Some(rule))
}

fn read_basic_rule(rpc: &str, message: &DynamicMessage) -> Result<HttpRule, DescriptorError> {
    let (method, pattern) = read_pattern(rpc, message)?;
    Ok(HttpRule::new(rpc, method, pattern)
        .with_body(read_body(message))
        .with_response_body(read_response_body(message)))
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
        let kind = field_str(custom, "kind").unwrap_or_default().to_owned();
        let path = field_str(custom, "path").unwrap_or_default().to_owned();
        return Ok((HttpMethod::Custom(kind), path));
    }

    Err(DescriptorError::no_pattern(rpc))
}

fn read_body(message: &DynamicMessage) -> Body {
    match field_str(message, "body") {
        Some("") | None => Body::None,
        Some("*") => Body::Whole,
        Some(field) => Body::Field(field.to_owned()),
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
    // `get_field_by_name` yields `Some(Cow::Borrowed(..))` for a field actually
    // stored on the message, and `Cow::Owned(default)` for an unset field; we
    // only report explicitly-present string values.
    match message.get_field_by_name(field) {
        Some(std::borrow::Cow::Borrowed(value)) => value.as_str(),
        _ => None,
    }
}

fn rust_type_path(full_name: &str, type_root: &str) -> String {
    let simple = full_name.rsplit('.').next().unwrap_or(full_name);
    format!("{type_root}::{simple}")
}

/// An error produced while reading service definitions from a descriptor set.
///
/// # Examples
///
/// ```no_run
/// # fn main() {
/// # #[cfg(feature = "descriptor")] {
/// use rest_over_grpc_build::{DescriptorError, services_from_descriptor};
///
/// let descriptor_set = std::fs::read("target/file_descriptor_set.bin")
///     .expect("the build script wrote a FileDescriptorSet");
/// let error: DescriptorError = services_from_descriptor(&descriptor_set, "crate::pb")
///     .expect_err("the descriptor set is invalid in this example");
///
/// assert!(!error.to_string().is_empty());
/// # }
/// # }
/// ```
#[derive(Debug)]
pub struct DescriptorError {
    kind: DescriptorErrorKind,
    #[expect(dead_code, reason = "captured for Debug output and RUST_BACKTRACE diagnostics")]
    backtrace: Box<Backtrace>,
}

#[derive(Debug)]
enum DescriptorErrorKind {
    Decode(String),
    Malformed { rpc: String, detail: String },
    NoPattern { rpc: String },
    Streaming { method: String },
    Rule(RuleError),
}

impl DescriptorError {
    fn new(kind: DescriptorErrorKind) -> Self {
        Self {
            kind,
            backtrace: Box::new(Backtrace::capture()),
        }
    }

    fn decode(detail: &str) -> Self {
        Self::new(DescriptorErrorKind::Decode(detail.to_owned()))
    }

    fn malformed(rpc: &str, detail: &str) -> Self {
        Self::new(DescriptorErrorKind::Malformed {
            rpc: rpc.to_owned(),
            detail: detail.to_owned(),
        })
    }

    fn no_pattern(rpc: &str) -> Self {
        Self::new(DescriptorErrorKind::NoPattern { rpc: rpc.to_owned() })
    }

    fn streaming(method: &str) -> Self {
        Self::new(DescriptorErrorKind::Streaming { method: method.to_owned() })
    }

    fn rule(source: RuleError) -> Self {
        Self::new(DescriptorErrorKind::Rule(source))
    }
}

impl fmt::Display for DescriptorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            DescriptorErrorKind::Decode(detail) => {
                write!(f, "failed to decode the descriptor set: {detail}")
            }
            DescriptorErrorKind::Malformed { rpc, detail } => {
                write!(f, "RPC `{rpc}` has a malformed http annotation: {detail}")
            }
            DescriptorErrorKind::NoPattern { rpc } => {
                write!(f, "RPC `{rpc}` http annotation has no URL pattern")
            }
            DescriptorErrorKind::Streaming { method } => {
                write!(f, "method `{method}` is streaming, which cannot be transcoded to unary REST")
            }
            DescriptorErrorKind::Rule(source) => write!(f, "{source}"),
        }
    }
}

impl std::error::Error for DescriptorError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.kind {
            DescriptorErrorKind::Rule(source) => Some(source),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::{env, fs, process};

    use super::*;
    use crate::write_annotation_protos;

    static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

    fn scratch_dir(name: &str) -> PathBuf {
        let suffix = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("rest_over_grpc_build_tests")
            .join(format!("{name}-{}-{suffix}", process::id()));
        fs::create_dir_all(&dir).expect("scratch directory is created under target");
        dir
    }

    fn compile_descriptor(name: &str, source: &str, include_annotations: bool) -> Vec<u8> {
        let root = scratch_dir(name);
        let proto_name = format!("{name}.proto");
        fs::write(root.join(&proto_name), source).expect("test proto is written under target");

        let mut includes = vec![root.clone()];
        if include_annotations {
            let annotations_include = root.join("proto_include");
            write_annotation_protos(&annotations_include).expect("vendored annotation protos are written");
            includes.push(annotations_include);
        }
        let include_refs: Vec<_> = includes.iter().map(PathBuf::as_path).collect();

        let mut compiler = protox::Compiler::new(include_refs).expect("protox compiler initializes");
        compiler.include_imports(true);
        compiler.open_file(&proto_name).expect("test proto compiles");
        compiler.encode_file_descriptor_set()
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

    #[test]
    fn maps_type_paths() {
        assert_eq!(rust_type_path("library.GetShelfRequest", "crate::pb"), "crate::pb::GetShelfRequest");
        assert_eq!(rust_type_path("a.b.c.Message", "crate::pb"), "crate::pb::Message");
        assert_eq!(rust_type_path("NoPackage", "crate::pb"), "crate::pb::NoPackage");
    }

    #[test]
    fn decode_error_is_reported() {
        let err = services_from_descriptor(b"not a descriptor set", "crate::pb").expect_err("invalid descriptor bytes");
        assert!(err.to_string().contains("decode"));
    }

    #[test]
    fn reads_annotated_unary_services() {
        let services = services_from_descriptor(&valid_descriptor(), "crate::pb").expect("annotations are read");

        assert_eq!(services.len(), 1);
        let code = services[0].generate().to_string();
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
        assert!(code.contains("BodyKind :: Whole"));
        assert!(code.contains("BodyKind :: Field"));
        assert!(code.contains("ResponseBodyKind :: Field"));
        assert!(!code.contains("no_annotation"));
    }

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

        let services = services_from_descriptor(&descriptor, "crate::pb").expect("descriptor is valid");

        assert!(services.is_empty());
    }

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

        let err = services_from_descriptor(&descriptor, "crate::pb").expect_err("streaming is rejected");

        assert!(err.to_string().contains("streaming"));
        assert!(std::error::Error::source(&err).is_none());
    }

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

        let err = services_from_descriptor(&descriptor, "crate::pb").expect_err("pattern is required");

        assert!(err.to_string().contains("no URL pattern"));
        assert!(std::error::Error::source(&err).is_none());
    }

    #[test]
    fn surfaces_rule_lowering_errors() {
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

        let err = services_from_descriptor(&descriptor, "crate::pb").expect_err("bad template is rejected");

        assert!(err.to_string().contains("invalid path template"));
        assert!(std::error::Error::source(&err).is_some());
    }

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

        let err = read_http_rule("Malformed", &options, Some(&scalar_ext)).expect_err("non-message extension is malformed");

        assert!(err.to_string().contains("malformed http annotation"));
    }

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
        let mut fake_http = DynamicMessage::new(fake_desc);
        fake_http.set_field_by_name("get", Value::String("/v1/fake".into()));
        let mut options = DynamicMessage::new(options_desc);
        options.set_extension(&fake_ext, Value::Message(fake_http));

        let rule = read_http_rule("Fake", &options, Some(&fake_ext))
            .expect("fake annotation is valid")
            .expect("fake annotation exists");
        let routes = rule.lower().expect("fake route lowers");

        assert_eq!(routes.len(), 1);
    }

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
        let err = read_http_rule("BadGet", &bad_get_options, Some(&bad_get_ext)).expect_err("pattern type is rejected");
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
        let err = read_http_rule("BadCustom", &bad_custom_options, Some(&bad_custom_ext)).expect_err("custom pattern type is rejected");
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
        let err = read_http_rule("BadAdditional", &bad_additional_options, Some(&bad_additional_ext))
            .expect_err("additional binding type is rejected");
        assert!(err.to_string().contains("additional_binding is not a message"));
    }

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
        let get_rule = read_http_rule(get.name(), &get_options, Some(&http_ext))
            .expect("GET annotation is valid")
            .expect("GET annotation exists");
        let get_routes = get_rule.lower().expect("GET routes lower");
        assert_eq!(get_routes.len(), 2);
        assert_eq!(get_routes[0].body(), &Body::None);
        assert_eq!(get_routes[0].response_body(), &ResponseBody::Field("item".into()));

        let post = service
            .methods()
            .find(|method| method.name() == "Post")
            .expect("POST method is present");
        let post_options = post.options();
        let post_rule = read_http_rule(post.name(), &post_options, Some(&http_ext))
            .expect("POST annotation is valid")
            .expect("POST annotation exists");
        let post_routes = post_rule.lower().expect("POST routes lower");
        assert_eq!(post_routes[0].body(), &Body::Whole);
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
}
