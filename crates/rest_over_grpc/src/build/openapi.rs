// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::BTreeMap;

use http_path_template::{PathTemplate, Segment};
use prost_reflect::{EnumDescriptor, FieldDescriptor, Kind, MessageDescriptor};
use serde::Serialize;

use crate::build::request_body::RequestBody;
use crate::build::response_body::ResponseBody;
use crate::build::route::Route;

/// Metadata for the generated OpenAPI document that the `.proto` file cannot
/// carry: the API title and version (both required by the OpenAPI
/// specification) and optional server base URLs.
///
/// # Examples
///
/// ```
/// use rest_over_grpc::build::OpenApiInfo;
///
/// let info = OpenApiInfo::new("Library", "v1").add_server("https://api.example.test");
/// assert_eq!(info.title(), "Library");
/// ```
#[derive(Debug, Clone)]
pub struct OpenApiInfo {
    title: String,
    version: String,
    servers: Vec<String>,
}

impl OpenApiInfo {
    /// Creates document metadata with the given `title` and `version`.
    #[must_use]
    pub fn new(title: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            version: version.into(),
            servers: Vec::new(),
        }
    }

    /// Adds a server base URL to the document's `servers` list.
    #[must_use]
    pub fn add_server(mut self, url: impl Into<String>) -> Self {
        self.servers.push(url.into());
        self
    }

    /// The document title.
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// The document version.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    /// The configured server base URLs.
    #[must_use]
    pub fn servers(&self) -> &[String] {
        &self.servers
    }
}

/// Accumulates the paths and component schemas for one service's OpenAPI
/// document, carried on a [`ServiceDefinition`](crate::build::ServiceDefinition) when it
/// is decoded from a descriptor.
#[derive(Default, Clone, Debug)]
pub(crate) struct Builder {
    paths: BTreeMap<String, BTreeMap<String, Operation>>,
    schemas: BTreeMap<String, Schema>,
}

/// The loop-invariant filtering context threaded through
/// [`Builder::collect_query`]: the proto-name paths already bound as path
/// parameters (excluded from the query) and the single body field (if any).
struct QueryFilter<'a> {
    path_fields: &'a [Vec<String>],
    body_field: Option<&'a str>,
}

impl Builder {
    /// Adds one route as an OpenAPI operation, collecting any referenced schemas.
    pub(crate) fn add_operation(&mut self, route: &Route, input: &MessageDescriptor, output: &MessageDescriptor, streaming: bool) {
        let parameters = self.parameters(route, input);
        let request_body = self.request_body(route, input);
        let responses = self.responses(route, output, streaming);

        let operation = Operation {
            id: route.rpc().to_owned(),
            parameters,
            request_body,
            responses,
        };

        let path = openapi_path(route.template());
        let verb = route.method().as_str().to_ascii_lowercase();
        self.paths.entry(path).or_default().insert(verb, operation);
    }

    /// Builds the path and query parameters for a route.
    fn parameters(&mut self, route: &Route, input: &MessageDescriptor) -> Vec<Parameter> {
        let path_fields = template_field_paths(route.template());
        let mut parameters = Vec::new();

        for field_path in &path_fields {
            let schema = self
                .resolve_field(input, field_path)
                .unwrap_or_else(|| Schema::scalar("string", None));
            parameters.push(Parameter {
                name: field_path.join("."),
                location: "path",
                required: true,
                schema,
            });
        }

        // Query parameters are the remaining leaf fields. A whole-body binding
        // leaves none; a single body field excludes that field's subtree.
        let body_field = match route.body() {
            RequestBody::Whole => return parameters,
            RequestBody::Field(field) => Some(field.as_str()),
            RequestBody::None => None,
        };
        let filter = QueryFilter {
            path_fields: &path_fields,
            body_field,
        };
        self.collect_query(input, &[], &[], &filter, &mut parameters, &[input.full_name().to_owned()]);
        parameters
    }

    /// Recursively collects query parameters from `message`, tracking both the
    /// proto-name path (for exclusion) and the json-name path (for the emitted
    /// parameter name). `visited` is the chain of message full-names from the
    /// root request message down to (and including) `message`; a nested singular
    /// message already on that chain is a cycle and is not expanded (a recursive
    /// message has no finite query-parameter form), which keeps this terminating.
    fn collect_query(
        &mut self,
        message: &MessageDescriptor,
        proto_prefix: &[String],
        json_prefix: &[String],
        filter: &QueryFilter<'_>,
        parameters: &mut Vec<Parameter>,
        visited: &[String],
    ) {
        for field in message.fields() {
            let mut proto_path = proto_prefix.to_vec();
            proto_path.push(field.name().to_owned());

            if filter.path_fields.contains(&proto_path) {
                continue;
            }
            if proto_prefix.is_empty() && filter.body_field == Some(field.name()) {
                continue;
            }

            let mut json_path = json_prefix.to_vec();
            json_path.push(field.json_name().to_owned());

            // A non-well-known message field, captured once for the branches below.
            let nested_message = match field.kind() {
                Kind::Message(nested) if well_known_schema(nested.full_name()).is_none() => Some(nested),
                _ => None,
            };

            // A singular (non-repeated, non-map) message expands into nested
            // dotted query parameters.
            if let Some(nested) = &nested_message
                && !field.is_list()
                && !field.is_map()
            {
                // Cycle guard: a message reachable from itself (directly or
                // mutually) would otherwise recurse forever.
                if visited.iter().any(|name| name == nested.full_name()) {
                    continue;
                }
                let mut next_visited = visited.to_vec();
                next_visited.push(nested.full_name().to_owned());
                self.collect_query(nested, &proto_path, &json_path, filter, parameters, &next_visited);
                continue;
            }

            // Maps, and repeated non-well-known messages, have no query form.
            if field.is_map() {
                continue;
            }
            if field.is_list() && nested_message.is_some() {
                continue;
            }

            parameters.push(Parameter {
                name: json_path.join("."),
                location: "query",
                required: false,
                schema: self.field_schema(&field),
            });
        }
    }

    /// Builds the request body object for a route, if any.
    fn request_body(&mut self, route: &Route, input: &MessageDescriptor) -> Option<RequestBodyObject> {
        let schema = match route.body() {
            RequestBody::None => return None,
            RequestBody::Whole => self.message_ref(input),
            RequestBody::Field(field) => {
                let field_desc = input
                    .get_field_by_name(field)
                    .or_else(|| input.get_field_by_json_name(field))
                    .expect("body field is validated to exist on the request message by read_http_rule");
                self.field_schema(&field_desc)
            }
        };
        Some(RequestBodyObject {
            required: true,
            content: MediaType::json(schema),
        })
    }

    /// Builds the responses map for a route: a `200` and a `default` error.
    fn responses(&mut self, route: &Route, output: &MessageDescriptor, streaming: bool) -> BTreeMap<String, ResponseObject> {
        let payload = match route.response_body() {
            ResponseBody::Whole => self.message_ref(output),
            ResponseBody::Field(field) => {
                let field_desc = output
                    .get_field_by_name(field)
                    .or_else(|| output.get_field_by_json_name(field))
                    .expect("response_body field is validated to exist on the response message by read_http_rule");
                self.field_schema(&field_desc)
            }
        };

        let (schema, description) = if streaming {
            (
                Schema::array(payload),
                "Server-streamed responses (a JSON array by default; NDJSON or SSE via Accept).",
            )
        } else {
            (payload, "Successful response.")
        };

        let mut responses = BTreeMap::new();
        responses.insert(
            "200".to_owned(),
            ResponseObject {
                description,
                content: MediaType::json(schema),
            },
        );
        responses.insert(
            "default".to_owned(),
            ResponseObject {
                description: "An error, as a gRPC status mapped to an HTTP status.",
                content: MediaType::json(self.status_ref()),
            },
        );
        responses
    }

    /// Resolves a (proto-name) field path against `message` to the leaf field's
    /// schema, or `None` if any segment is missing.
    fn resolve_field(&mut self, message: &MessageDescriptor, field_path: &[String]) -> Option<Schema> {
        let (last, parents) = field_path.split_last()?;
        let mut current = message.clone();
        for segment in parents {
            let field = current.fields().find(|candidate| candidate.name() == segment)?;
            match field.kind() {
                Kind::Message(nested) => current = nested,
                _ => return None,
            }
        }
        let field = current.fields().find(|candidate| candidate.name() == last)?;
        Some(self.field_schema(&field))
    }

    /// The schema for a field, accounting for `repeated` and `map` cardinality.
    fn field_schema(&mut self, field: &FieldDescriptor) -> Schema {
        if field.is_map()
            && let Kind::Message(entry) = field.kind()
        {
            let value = entry.map_entry_value_field();
            return Schema::map(self.kind_schema(value.kind()));
        }
        let base = self.kind_schema(field.kind());
        if field.is_list() { Schema::array(base) } else { base }
    }

    /// The schema for a single (non-repeated, non-map) protobuf kind.
    fn kind_schema(&mut self, kind: Kind) -> Schema {
        match kind {
            Kind::Double => Schema::scalar("number", Some("double")),
            Kind::Float => Schema::scalar("number", Some("float")),
            Kind::Int32 | Kind::Sint32 | Kind::Sfixed32 => Schema::scalar("integer", Some("int32")),
            Kind::Uint32 | Kind::Fixed32 => Schema::scalar("integer", Some("uint32")),
            Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 => Schema::scalar("string", Some("int64")),
            Kind::Uint64 | Kind::Fixed64 => Schema::scalar("string", Some("uint64")),
            Kind::Bool => Schema::scalar("boolean", None),
            Kind::String => Schema::scalar("string", None),
            Kind::Bytes => Schema::scalar("string", Some("byte")),
            Kind::Message(message) => well_known_schema(message.full_name()).unwrap_or_else(|| self.message_ref(&message)),
            Kind::Enum(enumeration) => self.enum_ref(&enumeration),
        }
    }

    /// Returns a `$ref` to `message`'s component schema, building it on first use.
    fn message_ref(&mut self, message: &MessageDescriptor) -> Schema {
        let name = message.full_name().to_owned();
        if !self.schemas.contains_key(&name) {
            // Insert a placeholder first so recursive references resolve.
            self.schemas.insert(name.clone(), Schema::default());
            let built = self.build_message(message);
            self.schemas.insert(name.clone(), built);
        }
        Schema::reference(&name)
    }

    /// Builds the object schema for a message from its fields.
    fn build_message(&mut self, message: &MessageDescriptor) -> Schema {
        let mut properties = BTreeMap::new();
        for field in message.fields() {
            properties.insert(field.json_name().to_owned(), self.field_schema(&field));
        }
        Schema::object(properties)
    }

    /// Returns a `$ref` to `enumeration`'s component schema, building it on first use.
    fn enum_ref(&mut self, enumeration: &EnumDescriptor) -> Schema {
        let name = enumeration.full_name().to_owned();
        if !self.schemas.contains_key(&name) {
            let values = enumeration.values().map(|value| value.name().to_owned()).collect();
            self.schemas.insert(name.clone(), Schema::enumeration(values));
        }
        Schema::reference(&name)
    }

    /// Returns a `$ref` to the shared error `Status` schema, building it once.
    fn status_ref(&mut self) -> Schema {
        let name = "Status";
        if !self.schemas.contains_key(name) {
            let mut properties = BTreeMap::new();
            properties.insert("code".to_owned(), Schema::scalar("integer", Some("int32")));
            properties.insert("message".to_owned(), Schema::scalar("string", None));
            self.schemas.insert(name.to_owned(), Schema::object(properties));
        }
        Schema::reference(name)
    }

    /// Finalizes the document with `info`.
    fn finish(self, info: &OpenApiInfo) -> Document {
        Document {
            openapi: "3.1.0",
            info: Info {
                title: info.title().to_owned(),
                version: info.version().to_owned(),
            },
            servers: info.servers().iter().map(|url| Server { url: url.clone() }).collect(),
            paths: self.paths,
            components: Components { schemas: self.schemas },
        }
    }

    /// Renders the accumulated document to pretty-printed JSON, using `info` for
    /// the title, version, and servers. Borrows `self` so a stored builder can be
    /// rendered repeatedly (e.g. per generator configuration).
    pub(crate) fn render(&self, info: &OpenApiInfo) -> String {
        render(&self.clone().finish(info))
    }
}

/// Serializes a document to pretty-printed JSON.
///
/// The OpenAPI model is composed entirely of strings, numbers, booleans, and
/// maps/vectors of the same, none of which can fail to serialize, so this never
/// returns an error.
fn render(document: &Document) -> String {
    serde_json::to_string_pretty(document).expect("the OpenAPI model always serializes to JSON")
}

/// Returns the inline schema for a `google.protobuf.*` well-known type, or
/// `None` if `full_name` is not a specially-mapped well-known type.
fn well_known_schema(full_name: &str) -> Option<Schema> {
    let schema = match full_name {
        "google.protobuf.Timestamp" => Schema::scalar("string", Some("date-time")),
        "google.protobuf.Duration" | "google.protobuf.FieldMask" | "google.protobuf.StringValue" => Schema::scalar("string", None),
        "google.protobuf.DoubleValue" => Schema::scalar("number", Some("double")),
        "google.protobuf.FloatValue" => Schema::scalar("number", Some("float")),
        "google.protobuf.Int32Value" => Schema::scalar("integer", Some("int32")),
        "google.protobuf.UInt32Value" => Schema::scalar("integer", Some("uint32")),
        "google.protobuf.Int64Value" => Schema::scalar("string", Some("int64")),
        "google.protobuf.UInt64Value" => Schema::scalar("string", Some("uint64")),
        "google.protobuf.BoolValue" => Schema::scalar("boolean", None),
        "google.protobuf.BytesValue" => Schema::scalar("string", Some("byte")),
        "google.protobuf.Struct" | "google.protobuf.Empty" | "google.protobuf.Any" => Schema::scalar("object", None),
        "google.protobuf.ListValue" => Schema::array(Schema::default()),
        "google.protobuf.Value" => Schema::default(),
        _ => return None,
    };
    Some(schema)
}

/// Reconstructs the OpenAPI path string from a parsed path template.
#[cfg_attr(coverage_nightly, coverage(off))]
fn openapi_path(template: &PathTemplate) -> String {
    let mut path = String::new();
    for segment in template.segments() {
        path.push('/');
        match segment {
            Segment::Literal(literal) => path.push_str(literal),
            Segment::Single => path.push('*'),
            Segment::Rest => path.push_str("**"),
            Segment::Variable(variable) => {
                path.push('{');
                path.push_str(&variable.field_path().join("."));
                path.push('}');
            }
            // `Segment` is `#[non_exhaustive]`; current variants are all handled.
            _ => {}
        }
    }
    if let Some(verb) = template.verb() {
        path.push(':');
        path.push_str(verb);
    }
    path
}

/// The proto-name field paths captured by a template's variables.
fn template_field_paths(template: &PathTemplate) -> Vec<Vec<String>> {
    template
        .segments()
        .iter()
        .filter_map(|segment| match segment {
            Segment::Variable(variable) => Some(variable.field_path().to_vec()),
            // Literal / Single / Rest (and any future `#[non_exhaustive]` variant)
            // bind no field.
            _ => None,
        })
        .collect()
}

// --- OpenAPI 3.1 serialization model (only the subset that is emitted) ---

#[derive(Serialize)]
struct Document {
    openapi: &'static str,
    info: Info,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    servers: Vec<Server>,
    paths: BTreeMap<String, BTreeMap<String, Operation>>,
    components: Components,
}

#[derive(Serialize)]
struct Info {
    title: String,
    version: String,
}

#[derive(Serialize)]
struct Server {
    url: String,
}

#[derive(Serialize)]
struct Components {
    schemas: BTreeMap<String, Schema>,
}

#[derive(Serialize, Clone, Debug)]
struct Operation {
    #[serde(rename = "operationId")]
    id: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    parameters: Vec<Parameter>,
    #[serde(rename = "requestBody", skip_serializing_if = "Option::is_none")]
    request_body: Option<RequestBodyObject>,
    responses: BTreeMap<String, ResponseObject>,
}

#[derive(Serialize, Clone, Debug)]
struct Parameter {
    name: String,
    #[serde(rename = "in")]
    location: &'static str,
    required: bool,
    schema: Schema,
}

#[derive(Serialize, Clone, Debug)]
struct RequestBodyObject {
    required: bool,
    content: BTreeMap<String, MediaType>,
}

#[derive(Serialize, Clone, Debug)]
struct ResponseObject {
    description: &'static str,
    content: BTreeMap<String, MediaType>,
}

#[derive(Serialize, Clone, Debug)]
struct MediaType {
    schema: Schema,
}

impl MediaType {
    fn json(schema: Schema) -> BTreeMap<String, Self> {
        let mut content = BTreeMap::new();
        content.insert("application/json".to_owned(), Self { schema });
        content
    }
}

#[derive(Serialize, Default, Clone, Debug)]
struct Schema {
    #[serde(rename = "$ref", skip_serializing_if = "Option::is_none")]
    reference: Option<String>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    ty: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    items: Option<Box<Self>>,
    #[serde(rename = "additionalProperties", skip_serializing_if = "Option::is_none")]
    additional_properties: Option<Box<Self>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    properties: Option<BTreeMap<String, Self>>,
    #[serde(rename = "enum", skip_serializing_if = "Option::is_none")]
    enum_values: Option<Vec<String>>,
}

impl Schema {
    fn scalar(ty: &'static str, format: Option<&'static str>) -> Self {
        Self {
            ty: Some(ty),
            format,
            ..Self::default()
        }
    }

    fn array(items: Self) -> Self {
        Self {
            ty: Some("array"),
            items: Some(Box::new(items)),
            ..Self::default()
        }
    }

    fn map(value: Self) -> Self {
        Self {
            ty: Some("object"),
            additional_properties: Some(Box::new(value)),
            ..Self::default()
        }
    }

    fn object(properties: BTreeMap<String, Self>) -> Self {
        Self {
            ty: Some("object"),
            properties: Some(properties),
            ..Self::default()
        }
    }

    fn enumeration(values: Vec<String>) -> Self {
        Self {
            ty: Some("string"),
            enum_values: Some(values),
            ..Self::default()
        }
    }

    fn reference(name: &str) -> Self {
        Self {
            reference: Some(format!("#/components/schemas/{name}")),
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::{env, fs, process};

    use serde_json::Value;

    use super::*;
    use crate::build::DescriptorOptions;

    static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

    fn compile(source: &str) -> Vec<u8> {
        compile_files(&[("test.proto", source)])
    }

    fn compile_files(files: &[(&str, &str)]) -> Vec<u8> {
        let suffix = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("openapi_tests")
            .join(format!("{}-{suffix}", process::id()));
        fs::create_dir_all(&dir).expect("scratch dir");
        for (name, source) in files {
            fs::write(dir.join(name), source).expect("write proto");
        }

        let annotations = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("proto");
        let includes = [dir.as_path(), annotations.as_path()];
        let mut compiler = protox::Compiler::new(includes).expect("compiler");
        compiler.include_imports(true);
        for (name, _) in files {
            compiler.open_file(name).expect("proto compiles");
        }
        compiler.encode_file_descriptor_set()
    }

    fn info() -> OpenApiInfo {
        OpenApiInfo::new("Test", "v1")
    }

    /// The per-service OpenAPI documents for `bytes`, keyed by module name.
    fn specs_with(bytes: &[u8], options: &DescriptorOptions, info: &OpenApiInfo) -> Vec<(String, Value)> {
        let mut generator = crate::build::Generator::builder().emit_openapi_spec(Some(info.clone())).build();
        generator.add_all(crate::build::ServiceDefinition::from_fds(bytes, options).expect("decode"));
        generator
            .generate()
            .1
            .iter()
            .filter_map(|output| {
                output
                    .openapi_spec()
                    .map(|spec| (output.module_name().to_owned(), serde_json::from_str(spec).expect("valid json")))
            })
            .collect()
    }

    fn doc(source: &str) -> Value {
        doc_with(source, &DescriptorOptions::new(), &info())
    }

    fn doc_with(source: &str, options: &DescriptorOptions, info: &OpenApiInfo) -> Value {
        let mut specs = specs_with(&compile(source), options, info);
        assert_eq!(specs.len(), 1, "expected exactly one service document");
        specs.pop().expect("one doc").1
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn document_header_and_info() {
        let doc = doc(r#"
                syntax = "proto3";
                package test;
                import "google/api/annotations.proto";
                service S { rpc Get(Req) returns (Res) { option (google.api.http) = { get: "/v1/x" }; } }
                message Req {}
                message Res { string name = 1; }
            "#);
        assert_eq!(doc["openapi"], "3.1.0");
        assert_eq!(doc["info"]["title"], "Test");
        assert_eq!(doc["info"]["version"], "v1");
        assert!(doc.get("servers").is_none(), "empty servers are omitted");
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn servers_are_emitted_when_configured() {
        let info = OpenApiInfo::new("Test", "v1")
            .add_server("https://a.test")
            .add_server("https://b.test");
        assert_eq!(info.servers(), ["https://a.test".to_owned(), "https://b.test".to_owned()]);
        let doc = doc_with(
            r#"
                syntax = "proto3";
                package test;
                import "google/api/annotations.proto";
                service S { rpc Get(Req) returns (Res) { option (google.api.http) = { get: "/v1/x" }; } }
                message Req {}
                message Res {}
            "#,
            &DescriptorOptions::new(),
            &info,
        );
        assert_eq!(doc["servers"][0]["url"], "https://a.test");
        assert_eq!(doc["servers"][1]["url"], "https://b.test");
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn path_verb_and_operation_id() {
        let doc = doc(r#"
                syntax = "proto3";
                package test;
                import "google/api/annotations.proto";
                service S {
                  rpc CreateThing(Thing) returns (Thing) { option (google.api.http) = { post: "/v1/things" body: "*" }; }
                }
                message Thing { string name = 1; }
            "#);
        let op = &doc["paths"]["/v1/things"]["post"];
        assert_eq!(op["operationId"], "CreateThing");
        assert!(op.get("parameters").is_none(), "no path/query params");
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn path_parameters_resolve_field_types_and_default_to_string() {
        // Several path-template shapes as distinct RPCs, compiled once: a simple
        // required string, dotted paths resolving nested field types, and
        // unresolvable segments that fall back to `string`.
        let doc = doc(r#"
                syntax = "proto3";
                package test;
                import "google/api/annotations.proto";
                service S {
                  rpc Shelf(ShelfReq) returns (Resp) { option (google.api.http) = { get: "/v1/shelves/{shelf}" }; }
                  rpc Nested(NestedReq) returns (Resp) { option (google.api.http) = { get: "/v1/{params.org}/{params.count}" }; }
                  rpc Unres(UnresReq) returns (Resp) { option (google.api.http) = { get: "/v1/{missing}/{scalar.x}" }; }
                }
                message Resp {}
                message ShelfReq { string shelf = 1; }
                message NestedReq { Params params = 1; }
                message Params { string org = 1; int32 count = 2; }
                message UnresReq { string scalar = 1; }
            "#);

        // A single `{shelf}` path parameter is a required path-scoped string.
        let params = doc["paths"]["/v1/shelves/{shelf}"]["get"]["parameters"].as_array().expect("params");
        assert_eq!(params.len(), 1);
        assert_eq!(params[0]["name"], "shelf");
        assert_eq!(params[0]["in"], "path");
        assert_eq!(params[0]["required"], true);
        assert_eq!(params[0]["schema"]["type"], "string");

        // Dotted path parameters resolve to the nested field's type.
        let params = doc["paths"]["/v1/{params.org}/{params.count}"]["get"]["parameters"]
            .as_array()
            .expect("params");
        let org = params.iter().find(|p| p["name"] == "params.org").expect("org param");
        assert_eq!(org["schema"]["type"], "string");
        let count = params.iter().find(|p| p["name"] == "params.count").expect("count param");
        assert_eq!(count["schema"]["type"], "integer");
        assert_eq!(count["schema"]["format"], "int32");

        // `{missing}` has no matching field and `{scalar.x}` walks through a
        // scalar; both default to `string`.
        let params = doc["paths"]["/v1/{missing}/{scalar.x}"]["get"]["parameters"]
            .as_array()
            .expect("params");
        for p in params {
            assert_eq!(p["schema"]["type"], "string", "{p:?}");
        }
    }
    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn query_parameters_use_json_names_and_nesting() {
        let doc = doc(r#"
                syntax = "proto3";
                package test;
                import "google/api/annotations.proto";
                service S { rpc List(Req) returns (Res) { option (google.api.http) = { get: "/v1/items" }; } }
                message Req { string free_text = 1; Page page = 2; }
                message Page { int32 page_size = 1; }
                message Res {}
            "#);
        let params = doc["paths"]["/v1/items"]["get"]["parameters"].as_array().expect("params");
        let text = params.iter().find(|p| p["name"] == "freeText").expect("freeText query");
        assert_eq!(text["in"], "query");
        assert_eq!(text["required"], false);
        assert_eq!(text["schema"]["type"], "string");
        let size = params.iter().find(|p| p["name"] == "page.pageSize").expect("nested query");
        assert_eq!(size["schema"]["type"], "integer");
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn query_excludes_path_and_body_fields() {
        let doc = doc(r#"
                syntax = "proto3";
                package test;
                import "google/api/annotations.proto";
                service S { rpc Update(Req) returns (Res) {
                  option (google.api.http) = { patch: "/v1/things/{id}" body: "payload" };
                } }
                message Req { string id = 1; Payload payload = 2; string extra = 3; }
                message Payload { string data = 1; }
                message Res {}
            "#);
        let op = &doc["paths"]["/v1/things/{id}"]["patch"];
        let params = op["parameters"].as_array().expect("params");
        let names: Vec<&str> = params.iter().map(|p| p["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"id"), "path param present");
        assert!(names.contains(&"extra"), "unbound field is a query param");
        assert!(!names.contains(&"payload"), "body field is not a query param");
        // The body binds the `payload` field specifically, so its schema is that
        // field's message, not some other field.
        assert_eq!(
            op["requestBody"]["content"]["application/json"]["schema"]["$ref"],
            "#/components/schemas/test.Payload"
        );
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn whole_body_binds_request_message_and_leaves_no_query() {
        let doc = doc(r#"
                syntax = "proto3";
                package test;
                import "google/api/annotations.proto";
                service S { rpc Create(Req) returns (Res) {
                  option (google.api.http) = { post: "/v1/things" body: "*" };
                } }
                message Req { string name = 1; string other = 2; }
                message Res {}
            "#);
        let op = &doc["paths"]["/v1/things"]["post"];
        assert!(op.get("parameters").is_none(), "whole body leaves no query params");
        assert_eq!(op["requestBody"]["required"], true);
        assert_eq!(
            op["requestBody"]["content"]["application/json"]["schema"]["$ref"],
            "#/components/schemas/test.Req"
        );
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn unknown_body_field_is_rejected() {
        // A `body` naming a nonexistent field is rejected at generation time.
        let error = crate::build::ServiceDefinition::from_fds(
            compile(
                r#"
                syntax = "proto3";
                package test;
                import "google/api/annotations.proto";
                service S { rpc Create(Req) returns (Res) {
                  option (google.api.http) = { post: "/v1/things" body: "nonexistent" };
                } }
                message Req { string name = 1; }
                message Res {}
            "#,
            ),
            &DescriptorOptions::new(),
        )
        .expect_err("unknown body field is rejected");
        assert!(error.to_string().contains("nonexistent"), "{error}");
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn get_has_no_request_body() {
        let doc = doc(r#"
                syntax = "proto3";
                package test;
                import "google/api/annotations.proto";
                service S { rpc Get(Req) returns (Res) { option (google.api.http) = { get: "/v1/x" }; } }
                message Req {}
                message Res {}
            "#);
        assert!(doc["paths"]["/v1/x"]["get"].get("requestBody").is_none());
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn response_body_whole_and_field() {
        let doc = doc(r#"
                syntax = "proto3";
                package test;
                import "google/api/annotations.proto";
                service S {
                  rpc Whole(Req) returns (Res) { option (google.api.http) = { get: "/v1/whole" }; }
                  rpc Sub(Req) returns (Res) { option (google.api.http) = { get: "/v1/sub" response_body: "thing" }; }
                }
                message Req {}
                message Res { Thing thing = 1; }
                message Thing { string name = 1; }
            "#);
        let whole = &doc["paths"]["/v1/whole"]["get"]["responses"]["200"]["content"]["application/json"]["schema"];
        assert_eq!(whole["$ref"], "#/components/schemas/test.Res");
        let sub = &doc["paths"]["/v1/sub"]["get"]["responses"]["200"]["content"]["application/json"]["schema"];
        assert_eq!(sub["$ref"], "#/components/schemas/test.Thing");
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn unknown_response_body_field_is_rejected() {
        // A `response_body` naming a nonexistent field is rejected at generation time.
        let error = crate::build::ServiceDefinition::from_fds(
            compile(
                r#"
                syntax = "proto3";
                package test;
                import "google/api/annotations.proto";
                service S {
                  rpc Missing(Req) returns (Res) { option (google.api.http) = { get: "/v1/missing" response_body: "nope" }; }
                }
                message Req {}
                message Res { Thing thing = 1; }
                message Thing { string name = 1; }
            "#,
            ),
            &DescriptorOptions::new(),
        )
        .expect_err("unknown response_body field is rejected");
        assert!(error.to_string().contains("nope"), "{error}");
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn server_streaming_response_is_an_array() {
        let doc = doc(r#"
                syntax = "proto3";
                package test;
                import "google/api/annotations.proto";
                service S { rpc Stream(Req) returns (stream Thing) { option (google.api.http) = { get: "/v1/things:watch" }; } }
                message Req {}
                message Thing { string name = 1; }
            "#);
        let response = &doc["paths"]["/v1/things:watch"]["get"]["responses"]["200"];
        let schema = &response["content"]["application/json"]["schema"];
        assert_eq!(schema["type"], "array");
        assert_eq!(schema["items"]["$ref"], "#/components/schemas/test.Thing");
        assert!(response["description"].as_str().unwrap().contains("NDJSON"));
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn error_response_references_status_schema() {
        let doc = doc(r#"
                syntax = "proto3";
                package test;
                import "google/api/annotations.proto";
                service S { rpc Get(Req) returns (Res) { option (google.api.http) = { get: "/v1/x" }; } }
                message Req {}
                message Res {}
            "#);
        let default = &doc["paths"]["/v1/x"]["get"]["responses"]["default"];
        assert_eq!(
            default["content"]["application/json"]["schema"]["$ref"],
            "#/components/schemas/Status"
        );
        let status = &doc["components"]["schemas"]["Status"];
        assert_eq!(status["type"], "object");
        assert_eq!(status["properties"]["code"]["type"], "integer");
        assert_eq!(status["properties"]["code"]["format"], "int32");
        assert_eq!(status["properties"]["message"]["type"], "string");
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "deliberately comprehensive: consolidates several schema cases into one proto compilation to keep test-suite (and mutation-testing) cost down"
    )]
    fn message_field_schemas_cover_scalars_enums_collections_recursion_and_well_known_types() {
        // One service with several response messages, compiled once, so the
        // per-message schema assertions share a single (costly) proto compilation.
        let doc = doc(r#"
                syntax = "proto3";
                package test;
                import "google/api/annotations.proto";
                import "google/protobuf/timestamp.proto";
                import "google/protobuf/duration.proto";
                import "google/protobuf/field_mask.proto";
                import "google/protobuf/wrappers.proto";
                import "google/protobuf/struct.proto";
                import "google/protobuf/empty.proto";
                import "google/protobuf/any.proto";
                service S {
                  rpc GetTypes(Req) returns (Types) { option (google.api.http) = { get: "/v1/types" }; }
                  rpc GetPalette(Req) returns (Palette) { option (google.api.http) = { get: "/v1/palette" }; }
                  rpc GetNode(Req) returns (Node) { option (google.api.http) = { get: "/v1/node" }; }
                  rpc GetWkt(Req) returns (W) { option (google.api.http) = { get: "/v1/wkt" }; }
                }
                message Req {}
                message Types {
                  double a_double = 1; float a_float = 2;
                  int32 a_i32 = 3; sint32 a_si32 = 4; sfixed32 a_sf32 = 5;
                  uint32 a_u32 = 6; fixed32 a_f32 = 7;
                  int64 a_i64 = 8; sint64 a_si64 = 9; sfixed64 a_sf64 = 10;
                  uint64 a_u64 = 11; fixed64 a_f64 = 12;
                  bool a_bool = 13; string a_string = 14; bytes a_bytes = 15;
                }
                enum Color { RED = 0; GREEN = 1; }
                message Palette {
                  Color color = 1;
                  Color accent = 2;
                  repeated string tags = 3;
                  map<string, int32> counts = 4;
                }
                message Node { string id = 1; Node parent = 2; repeated Node children = 3; }
                message W {
                  google.protobuf.Timestamp ts = 1;
                  google.protobuf.Duration dur = 2;
                  google.protobuf.FieldMask mask = 3;
                  google.protobuf.DoubleValue dv = 4;
                  google.protobuf.FloatValue fv = 5;
                  google.protobuf.Int32Value i32v = 6;
                  google.protobuf.UInt32Value u32v = 7;
                  google.protobuf.Int64Value i64v = 8;
                  google.protobuf.UInt64Value u64v = 9;
                  google.protobuf.BoolValue bv = 10;
                  google.protobuf.StringValue sv = 11;
                  google.protobuf.BytesValue bytesv = 12;
                  google.protobuf.Struct st = 13;
                  google.protobuf.Value val = 14;
                  google.protobuf.ListValue lst = 15;
                  google.protobuf.Empty empty = 16;
                  google.protobuf.Any any = 17;
                }
            "#);

        // Scalar fields map each proto scalar to its OpenAPI type/format.
        let props = &doc["components"]["schemas"]["test.Types"]["properties"];
        assert_eq!(props["aDouble"], serde_json::json!({"type":"number","format":"double"}));
        assert_eq!(props["aFloat"], serde_json::json!({"type":"number","format":"float"}));
        assert_eq!(props["aI32"], serde_json::json!({"type":"integer","format":"int32"}));
        assert_eq!(props["aSi32"], serde_json::json!({"type":"integer","format":"int32"}));
        assert_eq!(props["aSf32"], serde_json::json!({"type":"integer","format":"int32"}));
        assert_eq!(props["aU32"], serde_json::json!({"type":"integer","format":"uint32"}));
        assert_eq!(props["aF32"], serde_json::json!({"type":"integer","format":"uint32"}));
        assert_eq!(props["aI64"], serde_json::json!({"type":"string","format":"int64"}));
        assert_eq!(props["aSi64"], serde_json::json!({"type":"string","format":"int64"}));
        assert_eq!(props["aSf64"], serde_json::json!({"type":"string","format":"int64"}));
        assert_eq!(props["aU64"], serde_json::json!({"type":"string","format":"uint64"}));
        assert_eq!(props["aF64"], serde_json::json!({"type":"string","format":"uint64"}));
        assert_eq!(props["aBool"], serde_json::json!({"type":"boolean"}));
        assert_eq!(props["aString"], serde_json::json!({"type":"string"}));
        assert_eq!(props["aBytes"], serde_json::json!({"type":"string","format":"byte"}));

        // Enum, repeated, and map fields.
        let props = &doc["components"]["schemas"]["test.Palette"]["properties"];
        assert_eq!(props["color"]["$ref"], "#/components/schemas/test.Color");
        assert_eq!(props["accent"]["$ref"], "#/components/schemas/test.Color");
        assert_eq!(props["tags"], serde_json::json!({"type":"array","items":{"type":"string"}}));
        assert_eq!(
            props["counts"],
            serde_json::json!({"type":"object","additionalProperties":{"type":"integer","format":"int32"}})
        );
        let color = &doc["components"]["schemas"]["test.Color"];
        assert_eq!(color["type"], "string");
        assert_eq!(color["enum"], serde_json::json!(["RED", "GREEN"]));

        // A recursive message is emitted once and refers back to itself by `$ref`.
        let node = &doc["components"]["schemas"]["test.Node"];
        assert_eq!(node["properties"]["id"]["type"], "string");
        assert_eq!(node["properties"]["parent"]["$ref"], "#/components/schemas/test.Node");
        assert_eq!(node["properties"]["children"]["items"]["$ref"], "#/components/schemas/test.Node");

        // Well-known types map to their canonical OpenAPI representations.
        let props = &doc["components"]["schemas"]["test.W"]["properties"];
        assert_eq!(props["ts"], serde_json::json!({"type":"string","format":"date-time"}));
        assert_eq!(props["dur"], serde_json::json!({"type":"string"}));
        assert_eq!(props["mask"], serde_json::json!({"type":"string"}));
        assert_eq!(props["dv"], serde_json::json!({"type":"number","format":"double"}));
        assert_eq!(props["fv"], serde_json::json!({"type":"number","format":"float"}));
        assert_eq!(props["i32v"], serde_json::json!({"type":"integer","format":"int32"}));
        assert_eq!(props["u32v"], serde_json::json!({"type":"integer","format":"uint32"}));
        assert_eq!(props["i64v"], serde_json::json!({"type":"string","format":"int64"}));
        assert_eq!(props["u64v"], serde_json::json!({"type":"string","format":"uint64"}));
        assert_eq!(props["bv"], serde_json::json!({"type":"boolean"}));
        assert_eq!(props["sv"], serde_json::json!({"type":"string"}));
        assert_eq!(props["bytesv"], serde_json::json!({"type":"string","format":"byte"}));
        assert_eq!(props["st"], serde_json::json!({"type":"object"}));
        assert_eq!(props["val"], serde_json::json!({}));
        assert_eq!(props["lst"], serde_json::json!({"type":"array","items":{}}));
        assert_eq!(props["empty"], serde_json::json!({"type":"object"}));
        assert_eq!(props["any"], serde_json::json!({"type":"object"}));
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn well_known_type_as_query_parameter_is_inline() {
        let doc = doc(r#"
                syntax = "proto3";
                package test;
                import "google/api/annotations.proto";
                import "google/protobuf/timestamp.proto";
                service S { rpc List(Req) returns (Res) { option (google.api.http) = { get: "/v1/x" }; } }
                message Req { google.protobuf.Timestamp since = 1; }
                message Res {}
            "#);
        let params = doc["paths"]["/v1/x"]["get"]["parameters"].as_array().expect("params");
        let since = params.iter().find(|p| p["name"] == "since").expect("since query");
        assert_eq!(since["schema"], serde_json::json!({"type":"string","format":"date-time"}));
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn repeated_message_and_map_are_not_query_parameters() {
        let doc = doc(r#"
                syntax = "proto3";
                package test;
                import "google/api/annotations.proto";
                service S { rpc List(Req) returns (Res) { option (google.api.http) = { get: "/v1/x" }; } }
                message Req { repeated Nested items = 1; map<string, string> labels = 2; string keep = 3; }
                message Nested { string a = 1; }
                message Res {}
            "#);
        let op = &doc["paths"]["/v1/x"]["get"];
        let params = op["parameters"].as_array().expect("params");
        let names: Vec<&str> = params.iter().map(|p| p["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["keep"], "only the scalar leaf is a query param");
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn repeated_scalar_is_an_array_query_parameter() {
        let doc = doc(r#"
                syntax = "proto3";
                package test;
                import "google/api/annotations.proto";
                service S { rpc List(Req) returns (Res) { option (google.api.http) = { get: "/v1/x" }; } }
                message Req { repeated string tags = 1; }
                message Res {}
            "#);
        let params = doc["paths"]["/v1/x"]["get"]["parameters"].as_array().expect("params");
        let tags = params.iter().find(|p| p["name"] == "tags").expect("tags query");
        assert_eq!(tags["schema"], serde_json::json!({"type":"array","items":{"type":"string"}}));
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn path_renders_wildcards_and_verbs() {
        let doc = doc(r#"
                syntax = "proto3";
                package test;
                import "google/api/annotations.proto";
                service S {
                  rpc Single(Req) returns (Res) { option (google.api.http) = { get: "/v1/*/books" }; }
                  rpc Rest(Req) returns (Res) { option (google.api.http) = { get: "/v1/files/**" }; }
                  rpc Verb(Req) returns (Res) { option (google.api.http) = { get: "/v1/things:refresh" }; }
                }
                message Req {}
                message Res {}
            "#);
        let paths = doc["paths"].as_object().expect("paths");
        assert!(paths.contains_key("/v1/*/books"), "single wildcard: {paths:?}");
        assert!(paths.contains_key("/v1/files/**"), "rest wildcard");
        assert!(paths.contains_key("/v1/things:refresh"), "custom verb");
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn client_streaming_is_rejected() {
        let err = crate::build::ServiceDefinition::from_fds(
            compile(
                r#"
                    syntax = "proto3";
                    package test;
                    import "google/api/annotations.proto";
                    service S { rpc Up(stream Req) returns (Res) { option (google.api.http) = { post: "/v1/up" body: "*" }; } }
                    message Req {}
                    message Res {}
                "#,
            ),
            &DescriptorOptions::new(),
        )
        .expect_err("client streaming is rejected");
        assert!(err.to_string().contains("streaming"));
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn methods_without_http_rule_are_skipped() {
        let doc = doc(r#"
                syntax = "proto3";
                package test;
                import "google/api/annotations.proto";
                service S {
                  rpc Plain(Req) returns (Res);
                  rpc Annotated(Req) returns (Res) { option (google.api.http) = { get: "/v1/x" }; }
                }
                message Req {}
                message Res {}
            "#);
        let paths = doc["paths"].as_object().expect("paths");
        assert_eq!(paths.len(), 1);
        assert!(paths.contains_key("/v1/x"));
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn recursive_request_query_params_terminate() {
        // A self-referential request message reached via query parameters
        // terminates: the cycle is cut, yielding a finite parameter set.
        let doc = doc(r#"
                syntax = "proto3";
                package test;
                import "google/api/annotations.proto";
                service S { rpc Get(Node) returns (Res) { option (google.api.http) = { get: "/v1/nodes" }; } }
                message Node { string id = 1; Node parent = 2; string label = 3; }
                message Res {}
            "#);
        let params = doc["paths"]["/v1/nodes"]["get"]["parameters"].as_array().expect("query parameters");
        let names: Vec<&str> = params.iter().filter_map(|p| p["name"].as_str()).collect();
        // The message's own scalar fields are present...
        assert!(names.contains(&"id"), "{names:?}");
        assert!(names.contains(&"label"), "{names:?}");
        // ...but the self-referential `parent` field is not expanded (cycle cut).
        assert!(!names.iter().any(|n| n.starts_with("parent")), "{names:?}");
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn mutually_recursive_request_query_params_terminate() {
        // Mutual recursion `A -> B -> A` reached via query params must also
        // terminate, expanding one level of the cycle before cutting it.
        let doc = doc(r#"
                syntax = "proto3";
                package test;
                import "google/api/annotations.proto";
                service S { rpc Get(A) returns (Res) { option (google.api.http) = { get: "/v1/a" }; } }
                message A { string a_id = 1; B b = 2; }
                message B { string b_id = 1; A a = 2; }
                message Res {}
            "#);
        let params = doc["paths"]["/v1/a"]["get"]["parameters"].as_array().expect("query parameters");
        let names: Vec<&str> = params.iter().filter_map(|p| p["name"].as_str()).collect();
        assert!(names.contains(&"aId"), "{names:?}");
        assert!(names.contains(&"b.bId"), "{names:?}");
        // `b.a` would re-enter `A` (already on the path) and is cut.
        assert!(!names.iter().any(|n| n.starts_with("b.a")), "{names:?}");
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn one_document_per_package_and_filtering() {
        let one = (
            "one.proto",
            r#"
                syntax = "proto3";
                package one;
                import "google/api/annotations.proto";
                service A { rpc Get(R) returns (R) { option (google.api.http) = { get: "/one" }; } }
                message R {}
            "#,
        );
        let two = (
            "two.proto",
            r#"
                syntax = "proto3";
                package two;
                import "google/api/annotations.proto";
                service B { rpc Get(R) returns (R) { option (google.api.http) = { get: "/two" }; } }
                message R {}
            "#,
        );

        // Two packages compiled together yield one document each (module = package).
        let docs = specs_with(&compile_files(&[one, two]), &DescriptorOptions::new(), &info());
        assert_eq!(
            docs.iter().map(|(m, _)| m.clone()).collect::<Vec<_>>(),
            ["one".to_owned(), "two".to_owned()]
        );

        // Package filtering restricts to the requested package.
        let filtered = specs_with(&compile_files(&[one, two]), &DescriptorOptions::new().package(".one"), &info());
        assert_eq!(filtered.iter().map(|(m, _)| m.clone()).collect::<Vec<_>>(), ["one".to_owned()]);
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn two_services_in_one_package_produce_a_spec_each() {
        // Each service carries its own OpenAPI document (both under the same
        // module, since they share a package).
        let docs = specs_with(
            &compile(
                r#"
                    syntax = "proto3";
                    package test;
                    import "google/api/annotations.proto";
                    service A { rpc GetA(R) returns (R) { option (google.api.http) = { get: "/a" }; } }
                    service B { rpc GetB(R) returns (R) { option (google.api.http) = { get: "/b" }; } }
                    message R {}
                "#,
            ),
            &DescriptorOptions::new(),
            &info(),
        );
        assert_eq!(docs.len(), 2);
        assert!(docs.iter().all(|(module, _)| module == "test"));
        let has_path = |path: &str| {
            docs.iter()
                .any(|(_, doc)| doc["paths"].as_object().is_some_and(|paths| paths.contains_key(path)))
        };
        assert!(has_path("/a") && has_path("/b"));
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn invalid_descriptor_is_an_error() {
        let err = crate::build::ServiceDefinition::from_fds(b"not a descriptor set", &DescriptorOptions::new()).expect_err("decode fails");
        assert!(!err.to_string().is_empty());
    }

    #[cfg_attr(miri, ignore)] // proto compilation and filesystem I/O are unsupported under Miri.
    #[test]
    fn info_accessors() {
        let info = OpenApiInfo::new("T", "v9");
        assert_eq!(info.title(), "T");
        assert_eq!(info.version(), "v9");
        assert!(info.servers().is_empty());
    }
}
