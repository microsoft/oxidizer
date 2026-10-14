// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::{BTreeMap, HashSet};
use std::num::NonZeroU32;

use quick_xml::events::{BytesDecl, BytesStart, Event};
use quick_xml::name::{NamespaceError, PrefixDeclaration, QName, ResolveResult};
use quick_xml::reader::NsReader;
use quick_xml::{Error as XmlError, XmlVersion};
use uuid::Uuid;

use crate::Error;
use crate::error::configuration::{
    ControlCharacter, CounterCount, CounterPathCharacter, DuplicateName, EmptyField, IdCollision, InvalidXml, ManifestDefinition,
    ManifestSyntax, TextTooLong,
};

const EVENTS_NAMESPACE: &str = "http://schemas.microsoft.com/win/2004/08/events";
const COUNTERS_NAMESPACE: &str = "http://schemas.microsoft.com/win/2005/12/counters";
// Backend resource policies bound input, DOM storage, nesting, and native template storage.
pub(crate) const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
const MAX_XML_NODES: usize = 16_384;
const MAX_XML_DEPTH: usize = 64;
pub(crate) const MAX_COUNTERS: usize = 256;
// Windows manifest names are limited to 1023 UTF-16 units; help text has a backend limit.
const MAX_NAME: usize = 1023;
const MAX_TEXT: usize = 8192;
// PERF_COUNTERSET_INSTANCE followed by 64-bit reference slots in the supported native ABI.
const INSTANCE_HEADER_SIZE: u32 = 32;
const REFERENCE_SIZE: u32 = 8;

/// Parsed installation definitions; unsupported native profiles are checked when selecting a set.
#[derive(Clone, Debug)]
pub(crate) struct Manifest {
    providers: Vec<ManifestProvider>,
}

impl Manifest {
    /// Parses UTF-8 XML without consulting resources, the registry, or external entities.
    pub(crate) fn parse(xml: &str) -> Result<Self, Error> {
        if xml.len() > MAX_MANIFEST_BYTES {
            return Err(definition_error(format!("file exceeds {MAX_MANIFEST_BYTES} bytes")));
        }
        validate_xml_characters("manifest", xml)?;
        let root = parse_document(xml)?;
        if !root.is(EVENTS_NAMESPACE, "instrumentationManifest") {
            return Err(definition_error("expected the Microsoft instrumentationManifest root"));
        }
        let instrumentation = only_child(&root, EVENTS_NAMESPACE, "instrumentation")?;
        let counters = only_child(instrumentation, COUNTERS_NAMESPACE, "counters")?;
        let schema_version = counters.required("schemaVersion")?;
        let container_issue = profile_issue(counters, &["schemaVersion"], &["provider"]);
        let mut guids = HashSet::new();
        let providers = counters
            .children
            .iter()
            .filter(|element| element.is(COUNTERS_NAMESPACE, "provider"))
            .map(|element| parse_provider(element, schema_version, container_issue.as_deref(), &mut guids))
            .collect::<Result<Vec<_>, _>>()?;
        if providers.is_empty() {
            return Err(definition_error("counters must contain a direct provider definition"));
        }
        Ok(Self { providers })
    }
}

/// Validated layout shared with native registration, retaining installation IDs and source slots.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CounterSchema {
    pub(crate) category: String,
    pub(crate) provider_id: Uuid,
    pub(crate) counter_set_id: Uuid,
    pub(crate) counters: Vec<CounterDefinition>,
}

impl CounterSchema {
    /// Matches exact names at startup; unbound manifest counters receive distinct zero-backed slots.
    pub(crate) fn from_manifest(manifest: &Manifest, category: &str, instance: &str, specs: &[CounterSpec]) -> Result<Self, Error> {
        validate_display_name("category", category)?;
        validate_display_name("instance", instance)?;
        validate_count(specs.len())?;
        let mut matches = manifest.providers.iter().flat_map(|provider| {
            provider
                .sets
                .iter()
                .filter(|set| set.name.text == category)
                .map(move |set| (provider, set))
        });
        let (provider, set) = matches
            .next()
            .ok_or_else(|| definition_error(format!("no counter set named {category:?}")))?;
        if matches.next().is_some() {
            return Err(definition_error(format!(
                "counter set name {category:?} is ambiguous across providers"
            )));
        }
        provider.validate_profile()?;
        set.validate_profile()?;
        validate_count(set.counters.len())?;

        let mut bindings = BTreeMap::new();
        let mut indices = HashSet::new();
        for spec in specs {
            validate_display_name("counter name", &spec.name)?;
            if bindings.insert(spec.name.as_str(), spec.source_index).is_some() {
                return Err(Error::configuration(DuplicateName::new(spec.name.clone())));
            }
            if spec.source_index >= specs.len() || !indices.insert(spec.source_index) {
                return Err(definition_error("bound source indices must be a permutation of 0..bindings.len()"));
            }
        }

        let mut counters = set.counters.iter().collect::<Vec<_>>();
        counters.sort_unstable_by_key(|counter| counter.id);
        let mut next_unbound = specs.len();
        let mut next_offset = INSTANCE_HEADER_SIZE;
        let counters = counters
            .into_iter()
            .map(|counter| {
                counter.validate_profile()?;
                let name = counter.name.as_ref().ok_or_else(|| {
                    definition_error(format!(
                        "counter {} requires an inline name; resource IDs are not resolved",
                        counter.id
                    ))
                })?;
                let source_index = if let Some(index) = bindings.remove(name.text.as_str()) {
                    index
                } else {
                    let index = next_unbound;
                    next_unbound += 1;
                    index
                };
                // The selected set has at most MAX_COUNTERS slots, so this cannot overflow u32.
                let offset = next_offset;
                next_offset += REFERENCE_SIZE;
                Ok(CounterDefinition {
                    name: name.text.clone(),
                    description: counter.description.clone(),
                    id: counter.id,
                    offset,
                    source_index,
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        if let Some((name, _)) = bindings.into_iter().next() {
            return Err(definition_error(format!(
                "bound metric {name:?} does not exist in counter set {:?}",
                set.name.text
            )));
        }
        Ok(Self {
            category: set.name.text.clone(),
            provider_id: provider.id,
            counter_set_id: set.id,
            counters,
        })
    }
}

/// Builder input retaining the source-slot index of the bound metric.
#[derive(Clone, Debug)]
pub(crate) struct CounterSpec {
    pub(crate) name: String,
    pub(crate) source_index: usize,
}

/// Imported native counter with its declared ID, reference offset, and source-slot index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CounterDefinition {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) id: NonZeroU32,
    pub(crate) offset: u32,
    pub(crate) source_index: usize,
}

#[derive(Clone, Debug)]
struct ManifestProvider {
    id: Uuid,
    schema_version: String,
    provider_type: String,
    callback: String,
    issue: Option<String>,
    sets: Vec<ManifestCounterSet>,
}

impl ManifestProvider {
    fn validate_profile(&self) -> Result<(), Error> {
        validate_issue(self.issue.as_deref())?;
        if !matches!(self.schema_version.as_str(), "1.1" | "2.0") {
            return Err(definition_error("supported counters schemaVersion values are 1.1 and 2.0"));
        }
        if self.provider_type != "userMode" {
            return Err(definition_error("selected provider must use providerType=\"userMode\""));
        }
        if self.callback != "custom" {
            return Err(definition_error("selected provider must use callback=\"custom\""));
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
struct ManifestCounterSet {
    id: Uuid,
    name: DisplayName,
    instances: String,
    issue: Option<String>,
    counters: Vec<ManifestCounter>,
}

impl ManifestCounterSet {
    fn validate_profile(&self) -> Result<(), Error> {
        validate_issue(self.issue.as_deref())?;
        if self.instances != "multiple" {
            return Err(definition_error("selected counter set must use instances=\"multiple\""));
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
struct ManifestCounter {
    id: NonZeroU32,
    name: Option<DisplayName>,
    description: String,
    counter_type: Option<String>,
    detail_level: Option<String>,
    default_scale: Option<String>,
    flags: Vec<String>,
    issue: Option<String>,
}

impl ManifestCounter {
    fn validate_profile(&self) -> Result<(), Error> {
        validate_issue(self.issue.as_deref())?;
        if self.counter_type.as_deref() != Some("perf_counter_large_rawcount") {
            return Err(definition_error(format!(
                "counter {} must use type=\"perf_counter_large_rawcount\"",
                self.id
            )));
        }
        // counterman.xsd requires detailLevel; only defaultScale has a schema default.
        if self.detail_level.as_deref() != Some("standard") {
            return Err(definition_error(format!("counter {} requires detailLevel=\"standard\"", self.id)));
        }
        if let Some(scale) = &self.default_scale {
            let scale = scale.parse::<i32>().map_err(|cause| {
                Error::configuration(ManifestDefinition::caused_by("defaultScale must be an integer".to_owned(), cause))
            })?;
            if scale != 0 {
                return Err(definition_error(format!("counter {} must use defaultScale=\"0\"", self.id)));
            }
        }
        if self.flags.as_slice() != ["reference"] {
            return Err(definition_error(format!(
                "counter {} requires only the reference counterAttribute",
                self.id
            )));
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
struct DisplayName {
    text: String,
    folded: String,
}

impl DisplayName {
    fn parse(field: &'static str, text: &str) -> Result<Self, Error> {
        validate_display_name(field, text)?;
        Ok(Self {
            text: text.to_owned(),
            folded: folded_name(text),
        })
    }
}

fn parse_provider(
    element: &XmlElement,
    schema_version: &str,
    container_issue: Option<&str>,
    guids: &mut HashSet<Uuid>,
) -> Result<ManifestProvider, Error> {
    let id = parse_guid(element, "providerGuid", guids)?;
    let mut names = HashSet::new();
    let sets = element
        .children
        .iter()
        .filter(|element| element.is(COUNTERS_NAMESPACE, "counterSet"))
        .map(|element| {
            let set = parse_counter_set(element, guids)?;
            if !names.insert(set.name.folded.clone()) {
                return Err(Error::configuration(DuplicateName::new(set.name.text)));
            }
            Ok(set)
        })
        .collect::<Result<Vec<_>, Error>>()?;
    Ok(ManifestProvider {
        id,
        schema_version: schema_version.to_owned(),
        provider_type: element.attribute("providerType").unwrap_or("userMode").to_owned(),
        callback: element.attribute("callback").unwrap_or("default").to_owned(),
        issue: container_issue.map(str::to_owned).or_else(|| {
            profile_issue(
                element,
                &[
                    "providerGuid",
                    "providerType",
                    "callback",
                    "symbol",
                    "applicationIdentity",
                    "providerName",
                    "resourceBase",
                ],
                &["counterSet"],
            )
        }),
        sets,
    })
}

fn parse_counter_set(element: &XmlElement, guids: &mut HashSet<Uuid>) -> Result<ManifestCounterSet, Error> {
    let id = parse_guid(element, "guid", guids)?;
    let name = DisplayName::parse("category", element.required("name")?)?;
    if let Some(description) = element.attribute("description") {
        validate_text("description", description, MAX_TEXT, false)?;
    }
    let mut names = HashSet::new();
    let mut ids = HashSet::new();
    let counters = element
        .children
        .iter()
        .filter(|element| element.is(COUNTERS_NAMESPACE, "counter"))
        .map(|element| {
            let counter = parse_counter(element)?;
            if !ids.insert(counter.id) {
                return Err(Error::configuration(IdCollision::new(format!("counter {}", counter.id))));
            }
            if let Some(name) = &counter.name
                && !names.insert(name.folded.clone())
            {
                return Err(Error::configuration(DuplicateName::new(name.text.clone())));
            }
            Ok(counter)
        })
        .collect::<Result<Vec<_>, Error>>()?;
    Ok(ManifestCounterSet {
        id,
        name,
        instances: element.attribute("instances").unwrap_or("single").to_owned(),
        issue: profile_issue(
            element,
            &[
                "guid",
                "name",
                "description",
                "symbol",
                "uri",
                "nameID",
                "descriptionID",
                "instances",
            ],
            &["counter"],
        ),
        counters,
    })
}

fn parse_counter(element: &XmlElement) -> Result<ManifestCounter, Error> {
    let id = parse_counter_id(element.required("id")?)?;
    let name = element
        .attribute("name")
        .map(|name| DisplayName::parse("counter name", name))
        .transpose()?;
    let description = element.attribute("description").unwrap_or("").to_owned();
    validate_text("description", &description, MAX_TEXT, false)?;
    let mut flags = Vec::new();
    let mut issue = profile_issue(
        element,
        &[
            "id",
            "name",
            "description",
            "symbol",
            "uri",
            "nameID",
            "descriptionID",
            "type",
            "detailLevel",
            "defaultScale",
        ],
        &["counterAttributes"],
    );
    let groups = element
        .children
        .iter()
        .filter(|element| element.is(COUNTERS_NAMESPACE, "counterAttributes"))
        .collect::<Vec<_>>();
    if groups.len() > 1 {
        return Err(definition_error("counter must not repeat counterAttributes"));
    }
    if let Some(group) = groups.first() {
        issue = issue.or_else(|| profile_issue(group, &[], &["counterAttribute"]));
        for flag in group
            .children
            .iter()
            .filter(|element| element.is(COUNTERS_NAMESPACE, "counterAttribute"))
        {
            let name = flag.required("name")?;
            if flags.iter().any(|other| other == name) {
                return Err(definition_error(format!("duplicate counterAttribute {name:?}")));
            }
            issue = issue.or_else(|| profile_issue(flag, &["name"], &[]));
            flags.push(name.to_owned());
        }
    }
    Ok(ManifestCounter {
        id,
        name,
        description,
        counter_type: element.attribute("type").map(str::to_owned),
        detail_level: element.attribute("detailLevel").map(str::to_owned),
        default_scale: element.attribute("defaultScale").map(str::to_owned),
        flags,
        issue,
    })
}

fn parse_guid(element: &XmlElement, attribute: &str, guids: &mut HashSet<Uuid>) -> Result<Uuid, Error> {
    let value = element.required(attribute)?;
    let id = Uuid::parse_str(value)
        .map_err(|cause| Error::configuration(ManifestDefinition::caused_by(format!("invalid {attribute} GUID {value:?}"), cause)))?;
    if value.len() != 38 || !value.starts_with('{') || !value.ends_with('}') || id.is_nil() {
        return Err(definition_error(format!("{attribute} must be a nonnil braced GUID")));
    }
    if !guids.insert(id) {
        return Err(definition_error(format!("duplicate provider or counter-set GUID {id}")));
    }
    Ok(id)
}

fn parse_counter_id(value: &str) -> Result<NonZeroU32, Error> {
    // UInt32Type in counterman.xsd permits decimal or 0x/0X followed by 1..8 hex digits.
    let (digits, radix) = if let Some(hex) = value.strip_prefix("0x").or_else(|| value.strip_prefix("0X")) {
        // from_str_radix accepts a leading '+', which HexInt32Type does not permit.
        if hex.len() > 8 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(definition_error(format!("invalid hexadecimal counter ID {value:?}")));
        }
        (hex, 16)
    } else {
        if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(definition_error(format!("invalid decimal counter ID {value:?}")));
        }
        (value, 10)
    };
    let id = u32::from_str_radix(digits, radix)
        .map_err(|cause| Error::configuration(ManifestDefinition::caused_by(format!("invalid counter ID {value:?}"), cause)))?;
    if id == u32::MAX {
        return Err(definition_error("counter ID must not be PERF_WILDCARD_COUNTER (u32::MAX)"));
    }
    NonZeroU32::new(id).ok_or_else(|| definition_error("counter ID must not be zero"))
}

/// Transient XML tree used only during preparation; namespace resolution is delegated to quick-xml.
#[derive(Debug)]
struct XmlElement {
    qualified_name: String,
    namespace: Option<String>,
    attributes: Vec<XmlAttribute>,
    children: Vec<Self>,
    has_text: bool,
}

impl XmlElement {
    fn is(&self, namespace: &str, local_name: &str) -> bool {
        self.namespace.as_deref() == Some(namespace) && QName(self.qualified_name.as_bytes()).local_name().as_ref() == local_name.as_bytes()
    }

    fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|attribute| attribute.namespace.is_none() && attribute.name == name)
            .map(|attribute| attribute.value.as_str())
    }

    fn required(&self, name: &str) -> Result<&str, Error> {
        self.attribute(name)
            .ok_or_else(|| definition_error(format!("{} requires attribute {name:?}", self.qualified_name)))
    }
}

#[derive(Debug)]
struct XmlAttribute {
    namespace: Option<String>,
    name: String,
    value: String,
}

fn parse_document(xml: &str) -> Result<XmlElement, Error> {
    let mut reader = NsReader::from_str(xml);
    reader.config_mut().check_comments = true;
    let mut stack: Vec<XmlElement> = Vec::new();
    let mut root = None;
    let mut declaration_allowed = true;
    let mut nodes = 0;
    loop {
        let event = reader.read_event().map_err(syntax_error)?;
        let closes_element = matches!(event, Event::End(_) | Event::Empty(_));
        match event {
            Event::Start(start) | Event::Empty(start) => {
                declaration_allowed = false;
                if root.is_some() {
                    return Err(definition_error("XML must contain exactly one root element"));
                }
                nodes += 1;
                if nodes > MAX_XML_NODES || stack.len() >= MAX_XML_DEPTH {
                    return Err(definition_error(format!(
                        "XML exceeds {MAX_XML_NODES} elements or depth {MAX_XML_DEPTH}"
                    )));
                }
                let name = decode_name(start.name(), reader.decoder())?;
                if start.name().prefix().is_some_and(|prefix| prefix.is_xmlns()) {
                    return Err(definition_error("xmlns cannot be used as an element prefix"));
                }
                let (namespace, _) = reader.resolver().resolve_element(start.name());
                let namespace = decode_namespace(namespace)?;
                let attributes = parse_attributes(&start, &reader)?;
                stack.push(XmlElement {
                    qualified_name: name,
                    namespace,
                    attributes,
                    children: Vec::new(),
                    has_text: false,
                });
            }
            Event::End(_) => {}
            Event::Text(text) => {
                declaration_allowed = false;
                let text = text.xml_content(XmlVersion::Implicit1_0).map_err(syntax_error)?;
                if text.contains("]]>") {
                    return Err(definition_error("]]> is forbidden outside CDATA"));
                }
                record_text(&mut stack, &text)?;
            }
            Event::CData(text) => {
                declaration_allowed = false;
                if stack.is_empty() {
                    return Err(definition_error("CDATA is not allowed outside the root"));
                }
                record_text(&mut stack, &text.xml_content(XmlVersion::Implicit1_0).map_err(syntax_error)?)?;
            }
            Event::GeneralRef(reference) => {
                declaration_allowed = false;
                let name = reference.decode().map_err(syntax_error)?;
                let escaped = format!("&{name};");
                let text = quick_xml::escape::unescape_with(&escaped, quick_xml::escape::resolve_xml_entity).map_err(syntax_error)?;
                validate_xml_characters("manifest", &text)?;
                if stack.is_empty() {
                    return Err(definition_error("references are not allowed outside the root"));
                }
                record_text(&mut stack, &text)?;
            }
            Event::Decl(declaration) => {
                if !declaration_allowed {
                    return Err(definition_error("XML declaration must occur once, before any other content"));
                }
                validate_declaration(&declaration)?;
                declaration_allowed = false;
            }
            Event::DocType(_) => return Err(definition_error("DOCTYPE and external entities are not supported")),
            Event::PI(instruction) => {
                declaration_allowed = false;
                let target = decode_name(QName(instruction.target()), reader.decoder())?;
                if target.eq_ignore_ascii_case("xml") {
                    return Err(definition_error("processing instruction target xml is reserved"));
                }
            }
            Event::Comment(_) => declaration_allowed = false,
            Event::Eof => {
                if let Some(element) = stack.last() {
                    return Err(syntax_error(quick_xml::errors::IllFormedError::MissingEndTag(
                        element.qualified_name.clone(),
                    )));
                }
                return root.ok_or_else(|| definition_error("XML must contain a root element"));
            }
        }
        if closes_element {
            let element = stack.pop().ok_or_else(|| definition_error("closing tag has no opening tag"))?;
            if let Some(parent) = stack.last_mut() {
                parent.children.push(element);
            } else {
                root = Some(element);
            }
        }
    }
}

fn parse_attributes(start: &BytesStart<'_>, reader: &NsReader<&[u8]>) -> Result<Vec<XmlAttribute>, Error> {
    let mut attributes = Vec::new();
    let mut expanded_names = HashSet::new();
    for attribute in start.attributes() {
        let attribute = attribute.map_err(syntax_error)?;
        decode_name(attribute.key, reader.decoder())?;
        if attribute.value.contains(&b'<') {
            return Err(definition_error("literal < is forbidden in an attribute value"));
        }
        // Use the XML-only resolver even if a downstream dependency enables escape-html.
        let value = attribute
            .decoded_and_normalized_value_with(XmlVersion::Implicit1_0, reader.decoder(), 1, quick_xml::escape::resolve_xml_entity)
            .map_err(syntax_error)?;
        validate_xml_characters("manifest attribute", &value)?;
        if let Some(binding) = attribute.key.as_namespace_binding() {
            if matches!(binding, PrefixDeclaration::Named(_)) && value.is_empty() {
                return Err(definition_error("XML 1.0 forbids undeclaring a namespace prefix"));
            }
            continue;
        }
        let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
        let namespace = decode_namespace(namespace)?;
        let name = reader.decoder().decode(local.as_ref()).map_err(syntax_error)?.into_owned();
        if !expanded_names.insert((namespace.clone(), name.clone())) {
            return Err(definition_error(format!("duplicate expanded attribute {name:?}")));
        }
        attributes.push(XmlAttribute {
            namespace,
            name,
            value: value.into_owned(),
        });
    }
    Ok(attributes)
}

fn decode_namespace(namespace: ResolveResult<'_>) -> Result<Option<String>, Error> {
    match namespace {
        ResolveResult::Unbound => Ok(None),
        ResolveResult::Unknown(prefix) => Err(syntax_error(NamespaceError::UnknownPrefix(prefix))),
        ResolveResult::Bound(namespace) => {
            // NsReader resolves namespace bindings without decoding their attribute values.
            let attribute = quick_xml::events::attributes::Attribute::from((b"xmlns".as_slice(), namespace.as_ref()));
            let value = attribute
                .normalized_value_with(XmlVersion::Implicit1_0, 1, quick_xml::escape::resolve_xml_entity)
                .map_err(syntax_error)?;
            Ok(Some(value.into_owned()))
        }
    }
}

fn validate_declaration(declaration: &BytesDecl<'_>) -> Result<(), Error> {
    declaration.version().map_err(syntax_error)?;
    let start = BytesStart::from_content(
        std::str::from_utf8(declaration.as_ref()).map_err(|cause| syntax_error(quick_xml::encoding::EncodingError::Utf8(cause)))?,
        3,
    );
    let mut previous = None;
    for attribute in start.attributes() {
        let attribute = attribute.map_err(syntax_error)?;
        let (order, valid) = match attribute.key.as_ref() {
            b"version" => (0, attribute.value.as_ref() == b"1.0"),
            b"encoding" => (1, attribute.value.eq_ignore_ascii_case(b"utf-8")),
            b"standalone" => (2, matches!(attribute.value.as_ref(), b"yes" | b"no")),
            _ => return Err(definition_error("unsupported XML declaration attribute")),
        };
        if !valid || previous.is_some_and(|previous| order <= previous) {
            return Err(definition_error("expected an ordered XML 1.0 declaration with UTF-8 encoding"));
        }
        previous = Some(order);
    }
    Ok(())
}

fn decode_name(name: QName<'_>, decoder: quick_xml::encoding::Decoder) -> Result<String, Error> {
    let (local, prefix) = name.decompose();
    let local = decoder.decode(local.as_ref()).map_err(syntax_error)?;
    validate_ncname(&local)?;
    if let Some(prefix) = prefix {
        validate_ncname(&decoder.decode(prefix.as_ref()).map_err(syntax_error)?)?;
    }
    Ok(decoder.decode(name.as_ref()).map_err(syntax_error)?.into_owned())
}

fn validate_ncname(name: &str) -> Result<(), Error> {
    // quick-xml tokenizes names without enforcing the XML 1.0 NCName character production.
    let mut characters = name.chars();
    if !characters.next().is_some_and(name_start_character)
        || !characters.all(|character| {
            name_start_character(character)
                || matches!(character, '-' | '.' | '0'..='9' | '\u{b7}' | '\u{300}'..='\u{36f}' | '\u{203f}'..='\u{2040}')
        })
    {
        return Err(definition_error(format!("invalid XML qualified name {name:?}")));
    }
    Ok(())
}

fn name_start_character(character: char) -> bool {
    matches!(character, 'A'..='Z' | '_' | 'a'..='z' | '\u{c0}'..='\u{d6}' | '\u{d8}'..='\u{f6}' | '\u{f8}'..='\u{2ff}'
        | '\u{370}'..='\u{37d}' | '\u{37f}'..='\u{1fff}' | '\u{200c}'..='\u{200d}' | '\u{2070}'..='\u{218f}'
        | '\u{2c00}'..='\u{2fef}' | '\u{3001}'..='\u{d7ff}' | '\u{f900}'..='\u{fdcf}' | '\u{fdf0}'..='\u{fffd}'
        | '\u{10000}'..='\u{effff}')
}

fn record_text(stack: &mut [XmlElement], text: &str) -> Result<(), Error> {
    let non_whitespace = !text.chars().all(|character| matches!(character, ' ' | '\t' | '\r' | '\n'));
    if let Some(element) = stack.last_mut() {
        element.has_text |= non_whitespace;
    } else if non_whitespace {
        return Err(definition_error("text is not allowed outside the root"));
    }
    Ok(())
}

fn only_child<'a>(parent: &'a XmlElement, namespace: &str, name: &str) -> Result<&'a XmlElement, Error> {
    let mut children = parent.children.iter().filter(|element| element.is(namespace, name));
    let child = children
        .next()
        .ok_or_else(|| definition_error(format!("{} requires a direct {name} element", parent.qualified_name)))?;
    if children.next().is_some() {
        return Err(definition_error(format!("{} must not repeat {name}", parent.qualified_name)));
    }
    Ok(child)
}

fn profile_issue(element: &XmlElement, allowed_attributes: &[&str], allowed_children: &[&str]) -> Option<String> {
    if element.has_text {
        return Some(format!("{} cannot contain character data", element.qualified_name));
    }
    if let Some(attribute) = element
        .attributes
        .iter()
        .find(|attribute| attribute.namespace.is_some() || !allowed_attributes.contains(&attribute.name.as_str()))
    {
        return Some(format!("unsupported attribute {:?} on {}", attribute.name, element.qualified_name));
    }
    element
        .children
        .iter()
        .find(|child| !allowed_children.iter().any(|name| child.is(COUNTERS_NAMESPACE, name)))
        .map(|child| format!("unsupported child {} on {}", child.qualified_name, element.qualified_name))
}

fn validate_issue(issue: Option<&str>) -> Result<(), Error> {
    if let Some(issue) = issue {
        return Err(definition_error(issue));
    }
    Ok(())
}

fn definition_error(detail: impl Into<String>) -> Error {
    Error::configuration(ManifestDefinition::new(detail.into()))
}

fn syntax_error(cause: impl Into<XmlError>) -> Error {
    Error::configuration(ManifestSyntax::caused_by(cause.into()))
}

fn validate_count(count: usize) -> Result<(), Error> {
    if count == 0 || count > MAX_COUNTERS {
        return Err(Error::configuration(CounterCount::new(count, MAX_COUNTERS)));
    }
    Ok(())
}

fn folded_name(name: &str) -> String {
    // Conservatively include final sigma, sharp s, and dotless i case collisions.
    name.to_lowercase().to_uppercase()
}

fn validate_display_name(field: &'static str, value: &str) -> Result<(), Error> {
    validate_text(field, value, MAX_NAME, true)?;
    if value.contains(['\\', '/', '(', ')', '*', '?', '#']) {
        return Err(Error::configuration(CounterPathCharacter::new(field)));
    }
    Ok(())
}

fn validate_text(field: &'static str, value: &str, maximum: usize, required: bool) -> Result<(), Error> {
    if required && value.trim().is_empty() {
        return Err(Error::configuration(EmptyField::new(field)));
    }
    if value.encode_utf16().count() > maximum {
        return Err(Error::configuration(TextTooLong::new(field, maximum)));
    }
    validate_xml_characters(field, value)?;
    if required && value.chars().any(char::is_control) {
        return Err(Error::configuration(ControlCharacter::new(field)));
    }
    Ok(())
}

fn validate_xml_characters(field: &'static str, value: &str) -> Result<(), Error> {
    if value.chars().any(
        |character| !matches!(character, '\t' | '\n' | '\r' | '\u{20}'..='\u{d7ff}' | '\u{e000}'..='\u{fffd}' | '\u{10000}'..='\u{10ffff}'),
    ) {
        return Err(Error::configuration(InvalidXml::new(field)));
    }
    Ok(())
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use ohno::ErrorExt as _;

    use super::*;

    const XML: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<instrumentationManifest xmlns="http://schemas.microsoft.com/win/2004/08/events">
  <instrumentation>
    <counters xmlns="http://schemas.microsoft.com/win/2005/12/counters" schemaVersion="1.1">
      <provider providerGuid="{da724a43-2be1-487c-8a7a-ec9616ac9375}" callback="custom" providerType="userMode" applicationIdentity="service.dll">
        <counterSet guid="{d31325f4-7d84-40ac-9dc7-8237c9b101c7}" name="MyService" description="My service counters" instances="multiple">
          <counter id="90" name="Requests" description="requests received" type="perf_counter_large_rawcount" detailLevel="standard" defaultScale="0">
            <counterAttributes><counterAttribute name="reference"/></counterAttributes>
          </counter>
          <counter id="7" name="Bytes" description="bytes transferred" type="perf_counter_large_rawcount" detailLevel="standard" defaultScale="0">
            <counterAttributes><counterAttribute name="reference"/></counterAttributes>
          </counter>
          <counter id="21" name="Unused" type="perf_counter_large_rawcount" detailLevel="standard">
            <counterAttributes><counterAttribute name="reference"/></counterAttributes>
          </counter>
        </counterSet>
      </provider>
    </counters>
  </instrumentation>
</instrumentationManifest>"#;

    fn spec(name: &str, source_index: usize) -> CounterSpec {
        CounterSpec {
            name: name.to_owned(),
            source_index,
        }
    }

    fn schema(xml: &str, specs: &[CounterSpec]) -> Result<CounterSchema, Error> {
        CounterSchema::from_manifest(&Manifest::parse(xml)?, "MyService", "worker", specs)
    }

    fn definition_failure(xml: &str) -> Error {
        let error = schema(xml, &[spec("Requests", 0)]).unwrap_err();
        assert!(error.is_invalid_configuration());
        assert!(error.find_source::<ManifestDefinition>().is_some(), "{error:?}");
        error
    }

    #[test]
    fn imported_ids_guids_and_zero_slots_are_exact() {
        let actual = schema(XML, &[spec("Requests", 0), spec("Bytes", 1)]).unwrap();
        let expected = CounterSchema {
            category: "MyService".to_owned(),
            provider_id: Uuid::parse_str("da724a43-2be1-487c-8a7a-ec9616ac9375").unwrap(),
            counter_set_id: Uuid::parse_str("d31325f4-7d84-40ac-9dc7-8237c9b101c7").unwrap(),
            counters: vec![
                CounterDefinition {
                    name: "Bytes".to_owned(),
                    description: "bytes transferred".to_owned(),
                    id: NonZeroU32::new(7).unwrap(),
                    offset: 32,
                    source_index: 1,
                },
                CounterDefinition {
                    name: "Unused".to_owned(),
                    description: String::new(),
                    id: NonZeroU32::new(21).unwrap(),
                    offset: 40,
                    source_index: 2,
                },
                CounterDefinition {
                    name: "Requests".to_owned(),
                    description: "requests received".to_owned(),
                    id: NonZeroU32::new(90).unwrap(),
                    offset: 48,
                    source_index: 0,
                },
            ],
        };
        assert_eq!(actual, expected);
    }

    #[test]
    fn changing_binding_order_and_instance_keeps_installed_layout() {
        let manifest = Manifest::parse(XML).unwrap();
        let mut left = CounterSchema::from_manifest(&manifest, "MyService", "first", &[spec("Requests", 0), spec("Bytes", 1)]).unwrap();
        let right = CounterSchema::from_manifest(&manifest, "MyService", "second", &[spec("Bytes", 0), spec("Requests", 1)]).unwrap();
        for counter in &mut left.counters {
            if counter.source_index < 2 {
                counter.source_index = 1 - counter.source_index;
            }
        }
        assert_eq!(left, right);
    }

    #[test]
    fn binding_requires_the_exact_declared_instrument_name() {
        for (declared, bound) in [("Requests", "requests"), ("I", "ı"), ("ßσ", "ẞς")] {
            let xml = XML.replace(r#"name="Requests""#, &format!("name=\"{declared}\""));
            let error = schema(&xml, &[spec(bound, 0)]).unwrap_err();
            assert_eq!(
                error.find_source::<ManifestDefinition>().unwrap().detail,
                format!("bound metric {bound:?} does not exist in counter set \"MyService\""),
            );
            assert_eq!(schema(&xml, &[spec(declared, 0)]).unwrap().counters[2].name, declared);
        }
    }

    #[test]
    fn category_selection_requires_the_exact_manifest_name() {
        let error =
            CounterSchema::from_manifest(&Manifest::parse(XML).unwrap(), "MYSERVICE", "worker", &[spec("Requests", 0)]).unwrap_err();
        assert_eq!(
            error.find_source::<ManifestDefinition>().unwrap().detail,
            "no counter set named \"MYSERVICE\"",
        );
    }

    #[test]
    fn every_unbound_definition_has_a_unique_trailing_source_index() {
        let actual = schema(XML, &[spec("Bytes", 0)]).unwrap();
        assert_eq!(
            actual.counters.iter().map(|counter| counter.source_index).collect::<Vec<_>>(),
            [0, 1, 2]
        );
    }

    #[test]
    fn xml_attribute_entities_and_unicode_are_decoded_without_changing_ids() {
        let xml = XML
            .replace("Requests\"", "Sent &lt;&amp;&quot;&apos;&gt; 中文🦀\"")
            .replace("requests received", "line&#10;two&#13;&#9;&lt;&amp;&quot;&apos;&gt;");
        let actual = schema(&xml, &[spec("Sent <&\"'> 中文🦀", 0)]).unwrap();
        assert_eq!(
            (
                &actual.counters[2].name,
                &actual.counters[2].description,
                actual.counters[2].id.get()
            ),
            (&"Sent <&\"'> 中文🦀".to_owned(), &"line\ntwo\r\t<&\"'>".to_owned(), 90),
        );
    }

    #[test]
    fn default_namespace_and_prefixed_elements_have_the_same_meaning() {
        let mut prefixed = XML
            .replace(
                r#"xmlns="http://schemas.microsoft.com/win/2004/08/events""#,
                r#"xmlns:e="http://schemas.microsoft.com/win/2004/08/events""#,
            )
            .replace(
                r#"xmlns="http://schemas.microsoft.com/win/2005/12/counters""#,
                r#"xmlns:c="http://schemas.microsoft.com/win/2005/12/counters""#,
            );
        for name in ["instrumentationManifest", "instrumentation"] {
            prefixed = prefixed
                .replace(&format!("<{name}"), &format!("<e:{name}"))
                .replace(&format!("</{name}"), &format!("</e:{name}"));
        }
        for name in [
            "counterAttribute",
            "counterAttributes",
            "counterSet",
            "counters",
            "counter",
            "provider",
        ] {
            // Match the name boundary rather than rewriting already-prefixed child names.
            for ending in [" ", ">", "/"] {
                prefixed = prefixed.replace(&format!("<{name}{ending}"), &format!("<c:{name}{ending}"));
            }
            prefixed = prefixed.replace(&format!("</{name}>"), &format!("</c:{name}>"));
        }
        assert_eq!(
            schema(&prefixed, &[spec("Requests", 0)]).unwrap(),
            schema(XML, &[spec("Requests", 0)]).unwrap()
        );
    }

    #[test]
    fn escaped_namespace_uris_are_resolved() {
        let escaped = XML.replace("schemas.microsoft.com", "schemas.microso&#102;t.com");
        assert_eq!(
            schema(&escaped, &[spec("Bytes", 0)]).unwrap(),
            schema(XML, &[spec("Bytes", 0)]).unwrap()
        );
    }

    #[test]
    fn unrelated_sections_allow_comments_processing_instructions_cdata_and_xml_references() {
        let xml = XML
            .replace(
                "<instrumentation>",
                "<instrumentation><?process valid?><!-- valid -->\
                 <events>Unicode 中文 <![CDATA[unescaped <&>]]> &amp;&#65;&#x42;</events>",
            )
            .replace(
                "</instrumentationManifest>",
                "</instrumentationManifest><!-- after root --><?process after?>",
            );
        assert_eq!(
            schema(&xml, &[spec("Requests", 0)]).unwrap(),
            schema(XML, &[spec("Requests", 0)]).unwrap(),
        );
    }

    #[test]
    fn multiple_providers_and_sets_select_only_the_requested_definition() {
        let other_set = r#"<counterSet guid="{b44ee2a1-0aeb-4cdd-b560-a838bb2d0e8a}" name="Other" instances="single">
            <counter id="1" name="Other" type="perf_counter_timer"/>
        </counterSet>"#;
        let xml = XML.replace("</provider>", &format!("{other_set}</provider>")).replace(
            "</counters>",
            r#"<provider providerGuid="{298d1237-6cdb-4f67-b7fd-707c3ca33c92}" providerType="kernelMode">
                <counterSet guid="{38d80b95-66cb-4398-ad6a-81a00a6a53f8}" name="Kernel" instances="single"/>
            </provider></counters>"#,
        );
        assert_eq!(
            schema(&xml, &[spec("Requests", 0)]).unwrap(),
            schema(XML, &[spec("Requests", 0)]).unwrap()
        );
        let error = CounterSchema::from_manifest(&Manifest::parse(&xml).unwrap(), "Other", "worker", &[spec("Other", 0)]).unwrap_err();
        assert!(error.find_source::<ManifestDefinition>().unwrap().detail.contains("instances"));
    }

    #[test]
    fn unknown_and_ambiguous_counter_set_names_fail() {
        let error = CounterSchema::from_manifest(&Manifest::parse(XML).unwrap(), "Absent", "worker", &[spec("Requests", 0)]).unwrap_err();
        assert!(error.find_source::<ManifestDefinition>().unwrap().detail.contains("no counter set"));
        let xml = XML.replace(
            "</counters>",
            r#"<provider providerGuid="{298d1237-6cdb-4f67-b7fd-707c3ca33c92}" callback="custom">
            <counterSet guid="{38d80b95-66cb-4398-ad6a-81a00a6a53f8}" name="MyService" instances="multiple"/>
        </provider></counters>"#,
        );
        assert!(
            definition_failure(&xml)
                .find_source::<ManifestDefinition>()
                .unwrap()
                .detail
                .contains("ambiguous")
        );
    }

    #[test]
    fn mixed_etw_and_nested_spoofed_definitions_are_not_imported() {
        let xml = XML
            .replace(
                "<instrumentation>",
                r#"<instrumentation>
            <events><provider guid="not-a-counter-provider">
                <counterSet xmlns="http://schemas.microsoft.com/win/2005/12/counters" guid="invalid" name="MyService">
                    <counter id="0" name="Spoof"/>
                </counterSet>
            </provider></events>"#,
            )
            .replace(
                "</instrumentationManifest>",
                r#"<extension>
                <provider xmlns="http://schemas.microsoft.com/win/2005/12/counters" providerGuid="invalid"/>
            </extension></instrumentationManifest>"#,
            );
        assert_eq!(
            schema(&xml, &[spec("Requests", 0)]).unwrap(),
            schema(XML, &[spec("Requests", 0)]).unwrap()
        );
        let nested = XML
            .replace("<counterSet guid=", "<wrapper><counterSet guid=")
            .replace("</counterSet>", "</counterSet></wrapper>");
        definition_failure(&nested);
    }

    #[test]
    fn missing_inline_names_and_missing_bound_metrics_fail() {
        definition_failure(&XML.replace(r#"name="Unused""#, r#"nameID="123""#));
        let error = schema(XML, &[spec("Absent", 0)]).unwrap_err();
        assert!(error.find_source::<ManifestDefinition>().unwrap().detail.contains("bound metric"));
        definition_failure(&XML.replace(r#"name="MyService""#, r#"nameID="123""#));
    }

    #[test]
    fn missing_identity_attributes_and_direct_definitions_are_rejected() {
        for attribute in [
            r#"schemaVersion="1.1""#,
            r#"providerGuid="{da724a43-2be1-487c-8a7a-ec9616ac9375}""#,
            r#"guid="{d31325f4-7d84-40ac-9dc7-8237c9b101c7}""#,
            r#"id="90""#,
        ] {
            definition_failure(&XML.replace(attribute, ""));
        }
        let provider_start = XML.find("      <provider ").unwrap();
        let provider_end = XML.find("    </counters>").unwrap();
        definition_failure(&XML.replace(&XML[provider_start..provider_end], ""));
        definition_failure(&XML.replace(r#"name="reference""#, ""));
    }

    #[test]
    fn duplicate_bindings_and_invalid_source_indices_fail() {
        let error = schema(XML, &[spec("Requests", 0), spec("Requests", 1)]).unwrap_err();
        assert_eq!(error.find_source::<DuplicateName>().unwrap().name, "Requests");
        for specs in [
            vec![spec("Requests", 1)],
            vec![spec("Requests", 0), spec("Bytes", 0)],
            vec![spec("Bytes", usize::MAX)],
        ] {
            assert!(schema(XML, &specs).unwrap_err().find_source::<ManifestDefinition>().is_some());
        }
        assert_eq!(
            schema(XML, &[spec("Bytes", 1), spec("Requests", 0)])
                .unwrap()
                .counters
                .iter()
                .map(|counter| counter.source_index)
                .collect::<Vec<_>>(),
            [1, 2, 0],
        );
    }

    #[test]
    fn native_profile_rejects_types_flags_scale_detail_instances_and_provider_modes() {
        for (old, new) in [
            ("perf_counter_large_rawcount", "perf_counter_rawcount"),
            ("perf_counter_large_rawcount", "perf_counter_large_rawcount_hex"),
            ("perf_counter_large_rawcount", "perf_counter_timer"),
            (r#"name="reference""#, r#"name="displayAsHex""#),
            (r#"defaultScale="0""#, r#"defaultScale="1""#),
            (r#"defaultScale="0""#, r#"defaultScale="invalid""#),
            (r#"detailLevel="standard""#, r#"detailLevel="advanced""#),
            (r#"detailLevel="standard""#, ""),
            (r#"instances="multiple""#, r#"instances="single""#),
            (r#"instances="multiple""#, r#"instances="globalAggregate""#),
            (r#"instances="multiple""#, ""),
            (r#"providerType="userMode""#, r#"providerType="kernelMode""#),
            (r#"callback="custom""#, r#"callback="default""#),
            (r#"callback="custom""#, ""),
            (r#"schemaVersion="1.1""#, r#"schemaVersion="3.0""#),
        ] {
            definition_failure(&XML.replace(old, new));
        }
        definition_failure(&XML.replace("<counterAttributes><counterAttribute name=\"reference\"/></counterAttributes>", ""));
        definition_failure(&XML.replace(
            r#"<counterAttribute name="reference"/>"#,
            r#"<counterAttribute name="reference"/><counterAttribute name="noDisplay"/>"#,
        ));
        definition_failure(&XML.replace(
            r#"<counterAttribute name="reference"/>"#,
            r#"<counterAttribute name="reference"/><counterAttribute name="reference"/>"#,
        ));
    }

    #[test]
    fn schema_defaults_and_inline_metadata_are_supported() {
        let xml = XML
            .replace(r#" providerType="userMode""#, "")
            .replace(r#" defaultScale="0""#, "")
            .replace(r#"schemaVersion="1.1""#, r#"schemaVersion="2.0""#)
            .replace(
                r#"name="Requests""#,
                r#"name="Requests" nameID="100" descriptionID="101" symbol="REQUESTS" uri="urn:example:requests""#,
            );
        assert_eq!(
            schema(&xml, &[spec("Requests", 0)]).unwrap(),
            schema(XML, &[spec("Requests", 0)]).unwrap()
        );
    }

    #[test]
    fn unknown_semantics_are_not_silently_dropped() {
        for attribute in [
            "aggregate=\"sum\"",
            "baseID=\"7\"",
            "perfTimeID=\"7\"",
            "perfFreqID=\"7\"",
            "multiCounterID=\"7\"",
            "struct=\"Data\"",
            "field=\"Value\"",
            "typo=\"true\"",
        ] {
            definition_failure(&XML.replace(r#"id="90""#, &format!("id=\"90\" {attribute}")));
        }
        definition_failure(&XML.replace("<counterAttributes>", "<counterAttributes><extension/>"));
        definition_failure(&XML.replace(
            r#"<counterAttribute name="reference"/>"#,
            r#"<counterAttribute name="reference"><extension/></counterAttribute>"#,
        ));
        definition_failure(&XML.replace("<counterAttributes>", "text<counterAttributes>"));
        definition_failure(&XML.replace("<counterAttributes>", "<counterAttributes/><counterAttributes>"));
        for xml in [
            XML.replace(r#"schemaVersion="1.1""#, r#"schemaVersion="1.1" unknown="true""#),
            XML.replace(r#"callback="custom""#, r#"callback="custom" unknown="true""#),
            XML.replace(r#"instances="multiple""#, r#"instances="multiple" unknown="true""#),
            XML.replace("<counterSet guid=", "<extension/><counterSet guid="),
            XML.replace("<counterAttributes>", "<counterAttributes unknown=\"true\">"),
            XML.replace(r#"name="reference""#, r#"name="reference" unknown="true""#),
            XML.replace("<counterAttributes>", "<counterAttributes xmlns=\"urn:wrong\">"),
            XML.replace(
                r#"<counterAttribute name="reference"/>"#,
                r#"<counterAttribute name="reference">text</counterAttribute>"#,
            ),
        ] {
            definition_failure(&xml);
        }
    }

    #[test]
    fn manifest_ids_accept_decimal_and_hex_but_not_native_sentinels_or_invalid_numbers() {
        let xml = XML.replace(r#"id="90""#, r#"id="0X5A""#).replace(r#"id="7""#, r#"id="0x00000007""#);
        assert_eq!(
            schema(&xml, &[spec("Requests", 0)]).unwrap(),
            schema(XML, &[spec("Requests", 0)]).unwrap()
        );
        for id in [
            "0",
            "4294967295",
            "4294967296",
            "-1",
            "+1",
            "0x0",
            "0xFFFFFFFF",
            "0x100000000",
            "0x",
            "0xZZ",
            " 90 ",
            "1.0",
        ] {
            definition_failure(&XML.replace(r#"id="90""#, &format!("id=\"{id}\"")));
        }
        let upper = XML.replace(r#"id="90""#, r#"id="4294967294""#);
        assert_eq!(schema(&upper, &[spec("Requests", 0)]).unwrap().counters[2].id.get(), u32::MAX - 1);
    }

    #[test]
    fn zero_padded_ids_preserve_identity_but_obey_the_manifest_hexadecimal_width() {
        let expected = schema(XML, &[spec("Requests", 0)]).unwrap();
        for id in ["00000000000000090", "0x0000005A"] {
            let xml = XML.replace(r#"id="90""#, &format!("id=\"{id}\""));
            assert_eq!(schema(&xml, &[spec("Requests", 0)]).unwrap(), expected);
        }
        for id in ["0x00000005A", "0x+5A"] {
            let xml = XML.replace(r#"id="90""#, &format!("id=\"{id}\""));
            let error = Manifest::parse(&xml).unwrap_err();
            assert_eq!(
                error.find_source::<ManifestDefinition>().unwrap().detail,
                format!("invalid hexadecimal counter ID {id:?}"),
            );
        }
    }

    #[test]
    fn duplicate_guids_counter_ids_and_unicode_display_names_fail() {
        definition_failure(&XML.replace("{d31325f4-7d84-40ac-9dc7-8237c9b101c7}", "{da724a43-2be1-487c-8a7a-ec9616ac9375}"));
        let xml = XML.replace(
            "</counters>",
            r#"<provider providerGuid="{da724a43-2be1-487c-8a7a-ec9616ac9375}"/></counters>"#,
        );
        definition_failure(&xml);
        let xml = XML.replace(
            "</provider>",
            r#"<counterSet guid="{d31325f4-7d84-40ac-9dc7-8237c9b101c7}" name="Other"/></provider>"#,
        );
        definition_failure(&xml);
        let error = schema(&XML.replace(r#"id="90""#, r#"id="7""#), &[spec("Requests", 0)]).unwrap_err();
        assert!(error.find_source::<IdCollision>().is_some());
        for (left, right) in [("Requests", "REQUESTS"), ("ı", "I"), ("ẞς", "ßσ")] {
            let xml = XML
                .replace(r#"name="Requests""#, &format!("name=\"{left}\""))
                .replace(r#"name="Bytes""#, &format!("name=\"{right}\""));
            assert!(Manifest::parse(&xml).unwrap_err().find_source::<DuplicateName>().is_some());
        }
        let repeated_set = XML.replace(
            "</provider>",
            r#"<counterSet guid="{b44ee2a1-0aeb-4cdd-b560-a838bb2d0e8a}" name="MYSERVICE"/></provider>"#,
        );
        assert!(Manifest::parse(&repeated_set).unwrap_err().find_source::<DuplicateName>().is_some());
    }

    #[test]
    fn malformed_guid_errors_retain_the_uuid_cause() {
        let error = definition_failure(&XML.replace("{da724a43-2be1-487c-8a7a-ec9616ac9375}", "not-a-guid"));
        assert!(error.find_source::<uuid::Error>().is_some());
        for guid in ["{00000000-0000-0000-0000-000000000000}", "da724a43-2be1-487c-8a7a-ec9616ac9375"] {
            definition_failure(&XML.replace("{da724a43-2be1-487c-8a7a-ec9616ac9375}", guid));
        }
    }

    #[test]
    fn malformed_xml_keeps_quick_xml_causes_including_ignored_sections() {
        for xml in [
            XML[..XML.len() - "</instrumentationManifest>".len()].to_owned(),
            XML.replace("</provider>", "</wrong>"),
            XML.replace(r#"id="90""#, r#"id="90" id="91""#),
            XML.replace("<counterSet ", "<unbound:counterSet "),
            XML.replace(r#"id="90""#, r#"unbound:id="90""#),
            XML.replace("requests received", "&unknown;"),
            XML.replace("<instrumentation>", "<instrumentation><!-- bad -- comment -->"),
            XML.replace("<instrumentation>", "<instrumentation><events>&unknown;</events>"),
            XML.replace("requests received", "unterminated &"),
            XML.replace(r#"name="Requests""#, "name=Requests"),
            XML.replace("</instrumentationManifest>", "<unfinished"),
        ] {
            let error = Manifest::parse(&xml).unwrap_err();
            assert!(error.find_source::<ManifestSyntax>().is_some(), "{xml}\n{error:?}");
            assert!(error.find_source::<XmlError>().is_some());
        }
    }

    #[test]
    fn root_hierarchy_duplicate_attributes_and_document_content_are_strict() {
        for xml in [
            format!("{XML}{XML}"),
            format!("{XML}trailing"),
            format!("leading{XML}"),
            String::new(),
            XML.replace(EVENTS_NAMESPACE, "urn:wrong"),
            XML.replace("<instrumentation>", "<wrapper><instrumentation>")
                .replace("</instrumentation>", "</instrumentation></wrapper>"),
            XML.replace("<instrumentation>", "<instrumentation/><instrumentation>"),
            XML.replace(r#"id="90""#, r#"xmlns:a="urn:attrs" xmlns:b="urn:attrs" a:id="90" b:id="91""#),
            XML.replace(r#"name="Requests""#, r#"name="Requests<invalid""#),
            XML.replace("<instrumentation>", "<instrumentation><1invalid/>"),
            XML.replace("<instrumentation>", "<instrumentation><a:b:c xmlns:a=\"urn:attrs\"/>"),
            XML.replace("<instrumentation>", "<instrumentation><extension xmlns:a=\"\"/>"),
            XML.replace("<instrumentation>", "<instrumentation><xmlns:extension/>"),
            XML.replace("<instrumentation>", "<instrumentation><?XML reserved?>"),
            XML.replace("<instrumentation>", "<instrumentation>]]>"),
            format!("{XML}<![CDATA[]]>"),
            format!("{XML}&#32;"),
        ] {
            Manifest::parse(&xml).unwrap_err();
        }
    }

    #[test]
    fn a_second_root_without_an_xml_declaration_is_rejected() {
        let root_only = XML.replace("<?xml version=\"1.0\" encoding=\"utf-8\"?>", "");
        let error = Manifest::parse(&format!("{XML}{root_only}")).unwrap_err();
        assert_eq!(
            error.find_source::<ManifestDefinition>().unwrap().detail,
            "XML must contain exactly one root element",
        );
    }

    #[test]
    fn unicode_xml_element_names_in_unrelated_sections_preserve_counter_definitions() {
        let expected = schema(XML, &[spec("Requests", 0)]).unwrap();
        for name in [
            "Élément",
            "Øvelse",
            "øvelse",
            "\u{370}name",
            "Δεδομένα",
            "\u{200c}name",
            "\u{2070}name",
            "\u{2c00}name",
            "名前",
            "\u{f900}name",
            "\u{fdf0}name",
            "🦀name",
        ] {
            let xml = XML.replace("<instrumentation>", &format!("<instrumentation><events><{name}/></events>"));
            assert_eq!(schema(&xml, &[spec("Requests", 0)]).unwrap(), expected);
        }
    }

    #[test]
    fn doctype_external_entities_and_non_xml_characters_are_rejected() {
        for doctype in [
            "<!DOCTYPE instrumentationManifest>",
            "<!DOCTYPE instrumentationManifest SYSTEM \"file:///secret\">",
            "<!DOCTYPE instrumentationManifest [<!ENTITY custom \"expanded\">]>",
        ] {
            Manifest::parse(&XML.replace("<instrumentationManifest ", &format!("{doctype}<instrumentationManifest "))).unwrap_err();
        }
        for character in ["\0", "\u{1}", "\u{b}", "\u{ffff}", "&#0;", "&#1;", "&#xFFFF;"] {
            Manifest::parse(&XML.replace("requests received", character)).unwrap_err();
        }
    }

    #[test]
    fn xml_declaration_position_version_encoding_and_attribute_order_are_checked() {
        for declaration in [
            "<?xml?>",
            "<?xml encoding=\"utf-8\"?>",
            "<?xml version=\"1.1\"?>",
            "<?xml version=\"1.0\" encoding=\"utf-16\"?>",
            "<?xml version=\"1.0\" standalone=\"maybe\"?>",
            "<?xml version=\"1.0\" standalone=\"yes\" encoding=\"utf-8\"?>",
            "<?xml version=\"1.0\" unknown=\"true\"?>",
            "<?xml version=\"1.0\" version=\"1.0\"?>",
        ] {
            Manifest::parse(&XML.replace("<?xml version=\"1.0\" encoding=\"utf-8\"?>", declaration)).unwrap_err();
        }
        Manifest::parse(&format!(" {XML}")).unwrap_err();
        Manifest::parse(&format!("{XML}<?xml version=\"1.0\"?>")).unwrap_err();
        let xml = XML.replace(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>",
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>",
        );
        assert_eq!(
            schema(&xml, &[spec("Bytes", 0)]).unwrap(),
            schema(XML, &[spec("Bytes", 0)]).unwrap()
        );
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "large XML resource-limit breadth is covered natively and by cargo careful; ordinary parser and binding cases retain Miri coverage"
    )]
    fn file_byte_node_and_depth_budgets_have_exact_boundaries() {
        let exact = format!("{XML}{}", " ".repeat(MAX_MANIFEST_BYTES - XML.len()));
        Manifest::parse(&exact).unwrap();
        Manifest::parse(&format!("{exact} ")).unwrap_err();
        let base_nodes = 14;
        let exact_nodes = XML.replace(
            "</instrumentationManifest>",
            &format!("{}</instrumentationManifest>", "<extension/>".repeat(MAX_XML_NODES - base_nodes)),
        );
        Manifest::parse(&exact_nodes).unwrap();
        let extra_node = exact_nodes.replace("</instrumentationManifest>", "<extension/></instrumentationManifest>");
        Manifest::parse(&extra_node).unwrap_err();
        let deep = |depth| {
            XML.replace(
                "</instrumentationManifest>",
                &format!(
                    "{}{}</instrumentationManifest>",
                    "<extension>".repeat(depth - 1),
                    "</extension>".repeat(depth - 1)
                ),
            )
        };
        Manifest::parse(&deep(MAX_XML_DEPTH)).unwrap();
        Manifest::parse(&deep(MAX_XML_DEPTH + 1)).unwrap_err();
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "UTF-16/text policy boundaries are covered natively and by cargo careful; small Unicode and invalid-name cases retain Miri coverage"
    )]
    fn name_and_description_limits_use_utf16_units_and_keep_typed_errors() {
        let xml = XML
            .replace("Requests\"", &format!("{}\"", "🦀".repeat(511)))
            .replace("requests received", &"d".repeat(MAX_TEXT));
        schema(&xml, &[spec(&"🦀".repeat(511), 0)]).unwrap();
        for xml in [
            XML.replace("Requests\"", &format!("{}\"", "🦀".repeat(512))),
            XML.replace("requests received", &"d".repeat(MAX_TEXT + 1)),
        ] {
            assert!(Manifest::parse(&xml).unwrap_err().find_source::<TextTooLong>().is_some());
        }
    }

    #[test]
    fn invalid_display_names_retain_typed_configuration_causes() {
        let manifest = Manifest::parse(XML).unwrap();
        for (category, instance, field) in [(" ", "worker", "category"), ("MyService", "", "instance")] {
            let error = CounterSchema::from_manifest(&manifest, category, instance, &[spec("Requests", 0)]).unwrap_err();
            assert_eq!(error.find_source::<EmptyField>().unwrap().field, field);
        }
        let error = schema(XML, &[spec("line\nbreak", 0)]).unwrap_err();
        assert_eq!(error.find_source::<ControlCharacter>().unwrap().field, "counter name");
        for name in ["a/b", "a\\b", "(a)", "a*", "a?", "a#b"] {
            assert!(
                schema(XML, &[spec(name, 0)])
                    .unwrap_err()
                    .find_source::<CounterPathCharacter>()
                    .is_some()
            );
        }
        for name in [" ", "name&#10;break", "wild*card"] {
            Manifest::parse(&XML.replace("Requests\"", &format!("{name}\""))).unwrap_err();
        }
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "long display-name policy boundaries are covered natively and by cargo careful; ordinary exact-name binding retains Miri coverage"
    )]
    fn exact_display_name_limits_accept_both_manifest_and_binding_inputs() {
        let category = "c".repeat(MAX_NAME);
        let name = format!("{}x", "🦀".repeat(511));
        let xml = XML.replace("MyService", &category).replace("Requests\"", &format!("{name}\""));
        let manifest = Manifest::parse(&xml).unwrap();
        let actual = CounterSchema::from_manifest(&manifest, &category, &"i".repeat(MAX_NAME), &[spec(&name, 0)]).unwrap();
        assert_eq!(actual.category, category);
        let error = CounterSchema::from_manifest(&manifest, &category, &"i".repeat(MAX_NAME + 1), &[spec(&name, 0)]).unwrap_err();
        assert_eq!(error.find_source::<TextTooLong>().unwrap().field, "instance");
    }

    #[test]
    fn empty_selected_counter_sets_and_bindings_fail() {
        assert_eq!(schema(XML, &[]).unwrap_err().find_source::<CounterCount>().unwrap().count, 0);
        let empty = XML.replace(
            &XML[XML.find("          <counter id=").unwrap()..XML.find("        </counterSet>").unwrap()],
            "",
        );
        assert_eq!(
            schema(&empty, &[spec("Requests", 0)])
                .unwrap_err()
                .find_source::<CounterCount>()
                .unwrap()
                .count,
            0
        );
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "counter-count policy stress is covered natively and by cargo careful; layout, slot mapping, invalid indices, and empty schemas retain Miri coverage"
    )]
    fn oversized_selected_counter_sets_or_bindings_fail() {
        let counter = |id| {
            format!(
                r#"<counter id="{id}" name="Value {id}" type="perf_counter_large_rawcount" detailLevel="standard"><counterAttributes><counterAttribute name="reference"/></counterAttributes></counter>"#
            )
        };
        let extra = (100..100 + MAX_COUNTERS - 3).map(counter).collect::<String>();
        let exact = XML.replace("</counterSet>", &format!("{extra}</counterSet>"));
        assert_eq!(schema(&exact, &[spec("Requests", 0)]).unwrap().counters.len(), MAX_COUNTERS);
        let oversized = exact.replace("</counterSet>", &format!("{}</counterSet>", counter(999)));
        assert_eq!(
            schema(&oversized, &[spec("Requests", 0)])
                .unwrap_err()
                .find_source::<CounterCount>()
                .unwrap()
                .count,
            MAX_COUNTERS + 1
        );
        let specs = (0..=MAX_COUNTERS)
            .map(|index| spec(&format!("Value {index}"), index))
            .collect::<Vec<_>>();
        assert_eq!(
            schema(XML, &specs).unwrap_err().find_source::<CounterCount>().unwrap().count,
            MAX_COUNTERS + 1
        );
    }
}
