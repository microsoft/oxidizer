// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::num::NonZeroU32;

use observed::metadata::InstrumentKind;
use uuid::Uuid;

use crate::Error;
use crate::error::configuration::{
    ApplicationIdentity, ControlCharacter, CounterCount, CounterIndexOverflow, CounterPathCharacter, DuplicateName, EmptyField,
    IdCollision, InvalidXml, TextTooLong, UnsupportedAggregation,
};

// Fixed UUID namespace and format version make identities reproducible across
// processes, binding order, and toolchains. Changing the ABI requires a new version.
const NAMESPACE: Uuid = Uuid::from_u128(0x16a42c36_1b16_579f_bbea_96e29dba13d8);
// Installation-format prefix, independent of the Rust crate name.
// Changing it changes installed schema identities.
const ABI_VERSION: &[u8] = b"observed_windows/perflib-v2/u64-reference/v1";
// Conservative backend policy bounds template storage and collision-validation work.
pub(crate) const MAX_COUNTERS: usize = 256;
// Conservative backend policy bounds names and units in UTF-16 code units.
const MAX_NAME: usize = 1023;
// Conservative backend policy bounds descriptions and combined help text in UTF-16 code units.
const MAX_TEXT: usize = 8192;
// Conservative backend policy bounds resource filenames in UTF-16 code units.
const MAX_APPLICATION_IDENTITY: usize = 255;
// The native layout starts with PERF_COUNTERSET_INSTANCE, followed by pointer-sized
// reference slots in the supported ABI; slots hold pointers, not counter readings.
const INSTANCE_HEADER_SIZE: u32 = 32;
const REFERENCE_SIZE: u32 = 8;

/// Validated layout shared by native registration and deployment manifests.
#[derive(Clone, Debug)]
pub(crate) struct CounterSchema {
    pub(crate) category: String,
    pub(crate) provider_id: Uuid,
    pub(crate) counter_set_id: Uuid,
    pub(crate) counters: Vec<CounterDefinition>,
}

impl CounterSchema {
    /// Validates builder inputs and resolves native order while retaining source-slot indices.
    pub(crate) fn new(category: &str, instance: &str, specs: &[CounterSpec]) -> Result<Self, Error> {
        validate_text("category", category, MAX_NAME, true)?;
        validate_text("instance", instance, MAX_NAME, true)?;
        validate_name("category", category)?;
        validate_name("instance", instance)?;
        if specs.is_empty() || specs.len() > MAX_COUNTERS {
            return Err(Error::configuration(CounterCount::new(specs.len(), MAX_COUNTERS)));
        }

        let provider_id = identity(b"provider", category.as_bytes());
        let mut counters = specs
            .iter()
            .map(|spec| {
                validate_text("counter name", &spec.name, MAX_NAME, true)?;
                validate_name("counter name", &spec.name)?;
                validate_text("unit", &spec.unit, MAX_NAME, false)?;
                validate_text("description", &spec.description, MAX_TEXT, false)?;
                let counter = CounterDefinition {
                    name: spec.name.clone(),
                    unit: spec.unit.clone(),
                    description: spec.description.clone(),
                    id: counter_id(&spec.name),
                    offset: 0,
                    source_index: spec.source_index,
                    kind: spec.kind,
                };
                validate_text("counter help", &counter.help(), MAX_TEXT, false)?;
                Ok(counter)
            })
            .collect::<Result<Vec<_>, Error>>()?;
        counters.sort_unstable_by_key(|counter| counter.id);
        validate_collisions(&counters)?;

        let mut fingerprint = Vec::new();
        append_part(&mut fingerprint, category.as_bytes());
        for (index, counter) in counters.iter_mut().enumerate() {
            let index = u32::try_from(index).map_err(|_overflow| Error::configuration(CounterIndexOverflow::new()))?;
            counter.offset = INSTANCE_HEADER_SIZE + index * REFERENCE_SIZE;
            append_part(&mut fingerprint, &counter.id.get().to_be_bytes());
            append_part(&mut fingerprint, &counter.offset.to_be_bytes());
            append_part(&mut fingerprint, counter.name.as_bytes());
            append_part(&mut fingerprint, counter.unit.as_bytes());
            append_part(&mut fingerprint, counter.description.as_bytes());
            // Counters use the base fingerprint; other aggregation kinds append distinguishing
            // tags. Changing this encoding changes installed schema identities.
            match counter.kind {
                InstrumentKind::Counter => {}
                InstrumentKind::UpDownCounter => append_part(&mut fingerprint, b"updown-counter"),
                InstrumentKind::Gauge => append_part(&mut fingerprint, b"gauge"),
                _ => return Err(Error::configuration(UnsupportedAggregation::new(counter.kind))),
            }
        }
        let counter_set_id = identity(b"counter-set", &fingerprint);
        Ok(Self {
            category: category.into(),
            provider_id,
            counter_set_id,
            counters,
        })
    }

    /// Formats deployment definitions matching the native layout and stable identities.
    pub(crate) fn manifest(&self, application_identity: &str) -> Result<String, Error> {
        validate_text("application identity", application_identity, MAX_APPLICATION_IDENTITY, true)?;
        // A deployed binary filename, not an arbitrary path or command. lodctr
        // takes the binary directory separately, avoiding path interpretation.
        if application_identity.contains(['\\', '/', ':', '*', '?', '<', '>', '|', '"'])
            || ![".dll", ".exe"]
                .iter()
                .any(|extension| application_identity.to_ascii_lowercase().ends_with(extension))
        {
            return Err(Error::configuration(ApplicationIdentity::new()));
        }
        let provider = self.provider_id.braced();
        let counter_set = self.counter_set_id.braced();
        let category = xml_attribute(&self.category);
        let application = xml_attribute(application_identity);
        let mut xml = format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
             <instrumentationManifest xmlns=\"http://schemas.microsoft.com/win/2004/08/events\">\n\
             <instrumentation>\n\
             <counters xmlns=\"http://schemas.microsoft.com/win/2005/12/counters\" schemaVersion=\"1.1\">\n\
             <provider providerGuid=\"{provider}\" symbol=\"OBSERVED_PROVIDER\" providerType=\"userMode\" \
             callback=\"custom\" applicationIdentity=\"{application}\">\n\
             <counterSet guid=\"{counter_set}\" uri=\"urn:uuid:{}\" symbol=\"OBSERVED_COUNTER_SET\" \
             name=\"{category}\" description=\"{category} observed integer metrics\" instances=\"multiple\">\n",
            self.counter_set_id,
        );
        for counter in &self.counters {
            let name = xml_attribute(&counter.name);
            let help = xml_attribute(&counter.help());
            let id = counter.id;
            let counter_xml = format!(
                "<counter id=\"{id}\" uri=\"urn:uuid:{}:counter:{id}\" symbol=\"OBSERVED_COUNTER_{id}\" \
                 name=\"{name}\" description=\"{help}\" type=\"perf_counter_large_rawcount\" detailLevel=\"standard\" defaultScale=\"0\">\n\
                 <counterAttributes><counterAttribute name=\"reference\" /></counterAttributes>\n\
                 </counter>\n",
                self.counter_set_id,
            );
            xml.push_str(&counter_xml);
        }
        xml.push_str("</counterSet>\n</provider>\n</counters>\n</instrumentation>\n</instrumentationManifest>\n");
        Ok(xml)
    }
}

/// Builder input retaining the source-slot index of the bound metric.
#[derive(Clone, Debug)]
pub(crate) struct CounterSpec {
    pub(crate) name: String,
    pub(crate) unit: String,
    pub(crate) description: String,
    pub(crate) source_index: usize,
    pub(crate) kind: InstrumentKind,
}

/// Resolved native counter with its stable ID, reference offset, and source-slot index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CounterDefinition {
    pub(crate) name: String,
    pub(crate) unit: String,
    pub(crate) description: String,
    pub(crate) id: NonZeroU32,
    pub(crate) offset: u32,
    pub(crate) source_index: usize,
    pub(crate) kind: InstrumentKind,
}

impl CounterDefinition {
    /// Combines description and unit for deployment help text.
    fn help(&self) -> String {
        if self.unit.is_empty() {
            self.description.clone()
        } else {
            format!("{} [unit: {}]", self.description, self.unit)
        }
    }
}

/// Derives an installation identity from the versioned, domain-separated encoding.
fn identity(domain: &[u8], value: &[u8]) -> Uuid {
    let mut bytes = ABI_VERSION.to_vec();
    append_part(&mut bytes, domain);
    append_part(&mut bytes, value);
    Uuid::new_v5(&NAMESPACE, &bytes)
}

/// Length-prefixes each identity component so adjacent parts cannot be confused.
fn append_part(bytes: &mut Vec<u8>, part: &[u8]) {
    bytes.extend_from_slice(&(part.len() as u64).to_be_bytes());
    bytes.extend_from_slice(part);
}

/// Derives a stable counter ID excluding the zero and wildcard sentinels.
fn counter_id(name: &str) -> NonZeroU32 {
    let bytes = identity(b"counter-id", name.as_bytes()).into_bytes();
    // Counter IDs must not be zero or PERF_WILDCARD_COUNTER (u32::MAX).
    let id = (u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) & 0x7fff_ffff) + 1;
    NonZeroU32::new(id).expect("masking to 31 bits and adding one always yields a nonzero counter ID")
}

/// Rejects ID and case-normalized display-name collisions before native registration.
fn validate_collisions(counters: &[CounterDefinition]) -> Result<(), Error> {
    let mut folded_names = Vec::with_capacity(counters.len());
    for (index, counter) in counters.iter().enumerate() {
        if counters[..index].iter().any(|other| other.id == counter.id) {
            return Err(Error::configuration(IdCollision::new(counter.name.clone())));
        }
        // Conservatively reject Unicode case variants, including final sigma
        // and dotless i. Identity and resource text keep the original spelling.
        let folded = counter.name.to_lowercase().to_uppercase();
        if folded_names.iter().any(|other| other == &folded) {
            return Err(Error::configuration(DuplicateName::new(counter.name.clone())));
        }
        folded_names.push(folded);
    }
    Ok(())
}

/// Keeps names usable as literal components of Windows counter paths.
fn validate_name(field: &'static str, value: &str) -> Result<(), Error> {
    // These characters have structural meaning in PDH counter paths.
    if value.contains(['\\', '/', '(', ')', '*', '?', '#']) {
        return Err(Error::configuration(CounterPathCharacter::new(field)));
    }
    Ok(())
}

/// Enforces backend text limits and XML validity, with stricter rules for required names.
fn validate_text(field: &'static str, value: &str, maximum: usize, required: bool) -> Result<(), Error> {
    if required && value.trim().is_empty() {
        return Err(Error::configuration(EmptyField::new(field)));
    }
    if value.encode_utf16().count() > maximum {
        return Err(Error::configuration(TextTooLong::new(field, maximum)));
    }
    // XML 1.0 permits TAB/LF/CR but no other C0 controls or U+FFFE/U+FFFF.
    if value.chars().any(
        |character| !matches!(character, '\t' | '\n' | '\r' | '\u{20}'..='\u{d7ff}' | '\u{e000}'..='\u{fffd}' | '\u{10000}'..='\u{10ffff}'),
    ) {
        return Err(Error::configuration(InvalidXml::new(field)));
    }
    if required && value.chars().any(char::is_control) {
        return Err(Error::configuration(ControlCharacter::new(field)));
    }
    Ok(())
}

/// Escapes attribute text without losing Unicode or permitted whitespace.
fn xml_attribute(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            '\t' => escaped.push_str("&#9;"),
            '\n' => escaped.push_str("&#10;"),
            '\r' => escaped.push_str("&#13;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use ohno::ErrorExt as _;

    use super::*;

    fn spec(name: &str, index: usize) -> CounterSpec {
        CounterSpec {
            name: name.into(),
            unit: "By".into(),
            description: "bytes transferred".into(),
            source_index: index,
            kind: InstrumentKind::Counter,
        }
    }

    #[test]
    fn schema_rejects_unsupported_aggregation_and_name_controls() {
        let mut unsupported = spec("Value", 0);
        unsupported.kind = InstrumentKind::Histogram;
        let error = CounterSchema::new("Service", "worker", &[unsupported]).unwrap_err();
        let cause = error.find_source::<UnsupportedAggregation>().unwrap();
        assert_eq!(
            cause.message(),
            "invalid performance-counter configuration: unsupported native metric aggregation Histogram",
        );
        assert_eq!(cause.kind, InstrumentKind::Histogram);
        let error = CounterSchema::new("Service", "worker", &[spec("line\nbreak", 0)]).unwrap_err();
        let cause = error.find_source::<ControlCharacter>().unwrap();
        assert_eq!(
            cause.message(),
            "invalid performance-counter configuration: counter name contains a control character",
        );
        assert_eq!(cause.field, "counter name");
    }

    #[test]
    fn exact_text_limits_are_accepted_but_the_next_code_unit_is_rejected() {
        let schema = CounterSchema::new(&"s".repeat(MAX_NAME), &"i".repeat(MAX_NAME), &[spec(&"c".repeat(MAX_NAME), 0)]).unwrap();
        assert_eq!(schema.counters.len(), 1);
        let mut description = spec("Value", 0);
        description.unit.clear();
        description.description = "d".repeat(MAX_TEXT);
        CounterSchema::new("Service", "worker", &[description.clone()]).unwrap();
        description.description.push('d');
        let error = CounterSchema::new("Service", "worker", &[description]).unwrap_err();
        let cause = error.find_source::<TextTooLong>().unwrap();
        assert_eq!((cause.field, cause.maximum), ("description", MAX_TEXT));
    }

    #[test]
    fn a_valid_description_cannot_exceed_the_help_limit_after_appending_its_unit() {
        let mut counter = spec("Value", 0);
        counter.description = "d".repeat(MAX_TEXT);
        let error = CounterSchema::new("Service", "worker", &[counter]).unwrap_err();
        let cause = error.find_source::<TextTooLong>().unwrap();
        assert_eq!(
            cause.message(),
            "invalid performance-counter configuration: counter help exceeds 8192 UTF-16 code units",
        );
        assert_eq!((cause.field, cause.maximum), ("counter help", MAX_TEXT));
    }

    #[test]
    fn final_help_accepts_the_exact_utf16_limit_and_rejects_one_more_code_unit() {
        for unit in ["By", "🦀"] {
            let mut counter = spec("Value", 0);
            counter.unit = unit.into();
            let suffix = format!(" [unit: {unit}]");
            counter.description = "d".repeat(MAX_TEXT - suffix.encode_utf16().count());
            let expected_help = format!("{}{suffix}", counter.description);
            let schema = CounterSchema::new("Service", "worker", &[counter.clone()]).unwrap();
            assert_eq!(schema.counters[0].help(), expected_help);
            assert_eq!(schema.counters[0].help().encode_utf16().count(), MAX_TEXT);
            assert!(
                schema
                    .manifest("service.dll")
                    .unwrap()
                    .contains(&format!("description=\"{expected_help}\""))
            );
            counter.description.push('d');
            assert_eq!(
                CounterSchema::new("Service", "worker", &[counter])
                    .unwrap_err()
                    .find_source::<TextTooLong>()
                    .unwrap()
                    .message(),
                "invalid performance-counter configuration: counter help exceeds 8192 UTF-16 code units",
            );
        }
    }

    #[test]
    fn binding_order_and_instance_do_not_change_schema() {
        let left = CounterSchema::new("Service", "first", &[spec("Bytes", 0), spec("Requests", 1)]).unwrap();
        let right = CounterSchema::new("Service", "second", &[spec("Requests", 0), spec("Bytes", 1)]).unwrap();
        assert_eq!(left.manifest("service.dll").unwrap(), right.manifest("service.dll").unwrap());
    }

    #[test]
    fn installed_counter_identity_is_stable() {
        assert_eq!(counter_id("Bytes").get(), 1_414_435_675);
    }

    #[test]
    fn adding_a_counter_keeps_ids_but_changes_schema_identity() {
        let old = CounterSchema::new("Service", "worker", &[spec("Bytes", 0)]).unwrap();
        let new = CounterSchema::new("Service", "worker", &[spec("Bytes", 0), spec("Requests", 1)]).unwrap();
        assert_eq!(
            old.counters[0].id,
            new.counters.iter().find(|counter| counter.name == "Bytes").unwrap().id
        );
        assert_ne!(old.counter_set_id, new.counter_set_id);
        assert_eq!(old.provider_id, new.provider_id);
    }

    #[test]
    fn changing_unit_changes_schema_identity() {
        let old = CounterSchema::new("Service", "worker", &[spec("Bytes", 0)]).unwrap();
        let mut changed = spec("Bytes", 0);
        changed.unit = "KiBy".into();
        let new = CounterSchema::new("Service", "worker", &[changed]).unwrap();
        assert_ne!(old.counter_set_id, new.counter_set_id);
    }

    #[test]
    fn changing_aggregation_changes_schema_identity_but_keeps_counter_ids() {
        let counter = spec("Value", 0);
        let mut gauge = counter.clone();
        gauge.kind = InstrumentKind::Gauge;
        let mut updown = counter.clone();
        updown.kind = InstrumentKind::UpDownCounter;
        let counter = CounterSchema::new("Service", "worker", &[counter]).unwrap();
        let gauge = CounterSchema::new("Service", "worker", &[gauge]).unwrap();
        let updown = CounterSchema::new("Service", "worker", &[updown]).unwrap();
        assert_eq!(
            (counter.counters[0].id, counter.provider_id),
            (gauge.counters[0].id, gauge.provider_id)
        );
        assert_eq!(
            (counter.counters[0].id, counter.provider_id),
            (updown.counters[0].id, updown.provider_id)
        );
        assert_ne!(counter.counter_set_id, gauge.counter_set_id);
        assert_ne!(counter.counter_set_id, updown.counter_set_id);
        assert_ne!(gauge.counter_set_id, updown.counter_set_id);
    }

    #[test]
    fn xml_escaping_preserves_unicode_and_attribute_whitespace() {
        let mut counter = spec("Sent <&\"'> 中文🦀", 0);
        counter.description = "line one\nline two\r\t<&\"'>".into();
        let schema = CounterSchema::new("Category <&\"'> 中文🦀", "worker-🦀", &[counter]).unwrap();
        let xml = schema.manifest("resource&'.dll").unwrap();
        assert!(xml.contains("Category &lt;&amp;&quot;&apos;&gt; 中文🦀"));
        assert!(xml.contains("line one&#10;line two&#13;&#9;&lt;&amp;&quot;&apos;&gt; [unit: By]"));
        assert!(xml.contains("applicationIdentity=\"resource&amp;&apos;.dll\""));
    }

    #[test]
    fn invalid_names_are_rejected() {
        for value in [
            "",
            " ",
            "bad\0name",
            "bad\u{1}name",
            "bad\u{ffff}name",
            "a/b",
            "a\\b",
            "wild*",
            "(name)",
            "a#b",
        ] {
            assert!(CounterSchema::new(value, "worker", &[spec("Bytes", 0)]).is_err(), "{value:?}");
            assert!(CounterSchema::new("Service", value, &[spec("Bytes", 0)]).is_err(), "{value:?}");
            assert!(CounterSchema::new("Service", "worker", &[spec(value, 0)]).is_err(), "{value:?}");
        }
    }

    #[test]
    fn name_validation_retains_typed_field_context() {
        let empty = CounterSchema::new(" ", "worker", &[spec("Bytes", 0)]).unwrap_err();
        assert_eq!(empty.find_source::<EmptyField>().unwrap().field, "category");
        let invalid_xml = CounterSchema::new("Service", "worker", &[spec("bad\0name", 0)]).unwrap_err();
        assert_eq!(invalid_xml.find_source::<InvalidXml>().unwrap().field, "counter name");
        let counter_path = CounterSchema::new("Service", "worker", &[spec("bad\\name", 0)]).unwrap_err();
        assert_eq!(counter_path.find_source::<CounterPathCharacter>().unwrap().field, "counter name");
    }

    #[test]
    fn utf16_not_utf8_determines_name_limit() {
        let accepted = CounterSchema::new(&"🦀".repeat(511), "worker", &[spec("Bytes", 0)]);
        let rejected = CounterSchema::new(&"🦀".repeat(512), "worker", &[spec("Bytes", 0)]);
        accepted.unwrap();
        rejected.unwrap_err();
    }

    #[test]
    fn duplicate_names_and_ids_are_rejected() {
        let duplicate = CounterSchema::new("Service", "worker", &[spec("Bytes", 0), spec("BYTES", 1)]).unwrap_err();
        assert!(["Bytes", "BYTES"].contains(&duplicate.find_source::<DuplicateName>().unwrap().name.as_str()));
        CounterSchema::new("Service", "worker", &[spec("ı", 0), spec("I", 1)]).unwrap_err();
        CounterSchema::new("Service", "worker", &[spec("ẞς", 0), spec("ßσ", 1)]).unwrap_err();
        let mut schema = CounterSchema::new("Service", "worker", &[spec("Bytes", 0), spec("Requests", 1)]).unwrap();
        schema.counters[1].id = schema.counters[0].id;
        let collision = validate_collisions(&schema.counters).unwrap_err();
        assert_eq!(collision.find_source::<IdCollision>().unwrap().name, schema.counters[1].name);
    }

    #[test]
    fn empty_and_oversized_schemas_are_rejected() {
        let empty = CounterSchema::new("Service", "worker", &[]).unwrap_err();
        let cause = empty.find_source::<CounterCount>().unwrap();
        assert_eq!((cause.count, cause.maximum), (0, MAX_COUNTERS));
        let specs = (0..=MAX_COUNTERS)
            .map(|index| spec(&format!("Counter {index}"), index))
            .collect::<Vec<_>>();
        let oversized = CounterSchema::new("Service", "worker", &specs).unwrap_err();
        let cause = oversized.find_source::<CounterCount>().unwrap();
        assert_eq!((cause.count, cause.maximum), (MAX_COUNTERS + 1, MAX_COUNTERS));
    }

    #[test]
    fn binary_identity_must_be_a_filename() {
        let schema = CounterSchema::new("Service", "worker", &[spec("Bytes", 0)]).unwrap();
        for name in ["", "resources", "C:\\resources.dll", "../resources.dll", "bad\0.dll", "bad\".dll"] {
            assert!(schema.manifest(name).is_err(), "{name:?}");
        }
        let error = schema.manifest("C:\\resources.dll").unwrap_err();
        assert!(error.find_source::<ApplicationIdentity>().is_some());
    }
}
