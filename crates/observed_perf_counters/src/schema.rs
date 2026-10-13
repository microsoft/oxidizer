// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use observed::metadata::InstrumentKind;
use uuid::Uuid;

use crate::Error;

// Fixed UUID namespace and format version make identities reproducible across
// processes, binding order, and toolchains. Changing the ABI requires a new version.
const NAMESPACE: Uuid = Uuid::from_u128(0x16a42c36_1b16_579f_bbea_96e29dba13d8);
// Keep the original identity prefix so a crate rename does not change installed schemas.
const ABI_VERSION: &[u8] = b"observed_windows/perflib-v2/u64-reference/v1";
pub(crate) const MAX_COUNTERS: usize = 256;
const MAX_NAME: usize = 1023;
const MAX_TEXT: usize = 8192;
// PERF_COUNTERSET_INSTANCE is 32 bytes. A reference slot occupies eight bytes
// in our supported 64-bit ABI; it holds a pointer, not the counter data.
const INSTANCE_HEADER_SIZE: u32 = 32;
const REFERENCE_SIZE: u32 = 8;

#[derive(Clone, Debug)]
pub(crate) struct CounterSpec {
    pub(crate) name: String,
    pub(crate) unit: String,
    pub(crate) description: String,
    pub(crate) source_index: usize,
    pub(crate) kind: InstrumentKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CounterDefinition {
    pub(crate) name: String,
    pub(crate) unit: String,
    pub(crate) description: String,
    pub(crate) id: u32,
    pub(crate) offset: u32,
    pub(crate) source_index: usize,
    pub(crate) kind: InstrumentKind,
}

impl CounterDefinition {
    fn help(&self) -> String {
        if self.unit.is_empty() {
            self.description.clone()
        } else {
            format!("{} [unit: {}]", self.description, self.unit)
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CounterSchema {
    pub(crate) category: String,
    pub(crate) provider_id: Uuid,
    pub(crate) counter_set_id: Uuid,
    pub(crate) counters: Vec<CounterDefinition>,
}

impl CounterSchema {
    pub(crate) fn new(category: &str, instance: &str, specs: &[CounterSpec]) -> Result<Self, Error> {
        validate_text("category", category, MAX_NAME, true)?;
        validate_text("instance", instance, MAX_NAME, true)?;
        validate_name("category", category)?;
        validate_name("instance", instance)?;
        if specs.is_empty() || specs.len() > MAX_COUNTERS {
            return Err(Error::configuration(format!("bind between 1 and {MAX_COUNTERS} counters")));
        }

        let provider_id = identity(b"provider", category.as_bytes());
        let mut counters = specs
            .iter()
            .map(|spec| {
                validate_text("counter name", &spec.name, MAX_NAME, true)?;
                validate_name("counter name", &spec.name)?;
                validate_text("unit", &spec.unit, MAX_NAME, false)?;
                validate_text("description", &spec.description, MAX_TEXT, false)?;
                Ok(CounterDefinition {
                    name: spec.name.clone(),
                    unit: spec.unit.clone(),
                    description: spec.description.clone(),
                    id: counter_id(&spec.name),
                    offset: 0,
                    source_index: spec.source_index,
                    kind: spec.kind,
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        counters.sort_unstable_by_key(|counter| counter.id);
        validate_collisions(&counters)?;

        let mut fingerprint = Vec::new();
        append_part(&mut fingerprint, category.as_bytes());
        for (index, counter) in counters.iter_mut().enumerate() {
            let index = u32::try_from(index).map_err(|_overflow| Error::configuration("too many counters"))?;
            counter.offset = INSTANCE_HEADER_SIZE + index * REFERENCE_SIZE;
            append_part(&mut fingerprint, &counter.id.to_be_bytes());
            append_part(&mut fingerprint, &counter.offset.to_be_bytes());
            append_part(&mut fingerprint, counter.name.as_bytes());
            append_part(&mut fingerprint, counter.unit.as_bytes());
            append_part(&mut fingerprint, counter.description.as_bytes());
            // Preserve existing cumulative-counter identities; other aggregation
            // semantics must not reuse an installed counter's schema identity.
            match counter.kind {
                InstrumentKind::Counter => {}
                InstrumentKind::UpDownCounter => append_part(&mut fingerprint, b"updown-counter"),
                InstrumentKind::Gauge => append_part(&mut fingerprint, b"gauge"),
                _ => return Err(Error::configuration("unsupported native metric aggregation")),
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

    pub(crate) fn manifest(&self, application_identity: &str) -> Result<String, Error> {
        validate_text("application identity", application_identity, 255, true)?;
        // A deployed binary filename, not an arbitrary path or command. lodctr
        // takes the binary directory separately, avoiding path interpretation.
        if application_identity.contains(['\\', '/', ':', '*', '?', '<', '>', '|', '"'])
            || ![".dll", ".exe"]
                .iter()
                .any(|extension| application_identity.to_ascii_lowercase().ends_with(extension))
        {
            return Err(Error::configuration(
                "application identity must be a .dll or .exe filename without a directory",
            ));
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

fn identity(domain: &[u8], value: &[u8]) -> Uuid {
    let mut bytes = ABI_VERSION.to_vec();
    append_part(&mut bytes, domain);
    append_part(&mut bytes, value);
    Uuid::new_v5(&NAMESPACE, &bytes)
}

fn append_part(bytes: &mut Vec<u8>, part: &[u8]) {
    bytes.extend_from_slice(&(part.len() as u64).to_be_bytes());
    bytes.extend_from_slice(part);
}

fn counter_id(name: &str) -> u32 {
    let bytes = identity(b"counter-id", name.as_bytes()).into_bytes();
    // Counter IDs must not be zero or PERF_WILDCARD_COUNTER (u32::MAX).
    (u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) & 0x7fff_ffff) + 1
}

fn validate_collisions(counters: &[CounterDefinition]) -> Result<(), Error> {
    for (index, counter) in counters.iter().enumerate() {
        if counters[..index].iter().any(|other| other.id == counter.id) {
            return Err(Error::configuration(format!("counter ID collision for {:?}", counter.name)));
        }
        // Conservatively reject Unicode case variants, including final sigma
        // and dotless i. Identity and resource text keep the original spelling.
        let folded = counter.name.to_lowercase().to_uppercase();
        if counters[..index]
            .iter()
            .any(|other| other.name.to_lowercase().to_uppercase() == folded)
        {
            return Err(Error::configuration(format!("duplicate counter display name {:?}", counter.name)));
        }
    }
    Ok(())
}

fn validate_name(field: &str, value: &str) -> Result<(), Error> {
    // These characters have structural meaning in PDH counter paths.
    if value.contains(['\\', '/', '(', ')', '*', '?', '#']) {
        return Err(Error::configuration(format!(
            "{field} contains a Windows counter-path metacharacter"
        )));
    }
    Ok(())
}

fn validate_text(field: &str, value: &str, maximum: usize, required: bool) -> Result<(), Error> {
    if required && value.trim().is_empty() {
        return Err(Error::configuration(format!("{field} must not be empty")));
    }
    if value.encode_utf16().count() > maximum {
        return Err(Error::configuration(format!("{field} exceeds {maximum} UTF-16 code units")));
    }
    // XML 1.0 permits TAB/LF/CR but no other C0 controls or U+FFFE/U+FFFF.
    if value.chars().any(
        |character| !matches!(character, '\t' | '\n' | '\r' | '\u{20}'..='\u{d7ff}' | '\u{e000}'..='\u{fffd}' | '\u{10000}'..='\u{10ffff}'),
    ) {
        return Err(Error::configuration(format!("{field} contains NUL or an invalid XML character")));
    }
    if required && value.chars().any(char::is_control) {
        return Err(Error::configuration(format!("{field} contains a control character")));
    }
    Ok(())
}

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
        assert_eq!(
            CounterSchema::new("Service", "worker", &[unsupported]).unwrap_err().to_string(),
            "invalid performance-counter configuration: unsupported native metric aggregation",
        );
        assert_eq!(
            CounterSchema::new("Service", "worker", &[spec("line\nbreak", 0)])
                .unwrap_err()
                .to_string(),
            "invalid performance-counter configuration: counter name contains a control character",
        );
    }

    #[test]
    fn exact_text_limits_are_accepted_but_the_next_code_unit_is_rejected() {
        let schema = CounterSchema::new(&"s".repeat(MAX_NAME), &"i".repeat(MAX_NAME), &[spec(&"c".repeat(MAX_NAME), 0)]).unwrap();
        assert_eq!(schema.counters.len(), 1);
        let mut description = spec("Value", 0);
        description.description = "d".repeat(MAX_TEXT);
        CounterSchema::new("Service", "worker", &[description.clone()]).unwrap();
        description.description.push('d');
        CounterSchema::new("Service", "worker", &[description]).unwrap_err();
    }

    #[test]
    fn binding_order_and_instance_do_not_change_schema() {
        let left = CounterSchema::new("Service", "first", &[spec("Bytes", 0), spec("Requests", 1)]).unwrap();
        let right = CounterSchema::new("Service", "second", &[spec("Requests", 0), spec("Bytes", 1)]).unwrap();
        assert_eq!(left.manifest("service.dll").unwrap(), right.manifest("service.dll").unwrap());
    }

    #[test]
    fn installed_counter_identity_is_stable() {
        assert_eq!(counter_id("Bytes"), 1_414_435_675);
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
    fn utf16_not_utf8_determines_name_limit() {
        let accepted = CounterSchema::new(&"🦀".repeat(511), "worker", &[spec("Bytes", 0)]);
        let rejected = CounterSchema::new(&"🦀".repeat(512), "worker", &[spec("Bytes", 0)]);
        accepted.unwrap();
        rejected.unwrap_err();
    }

    #[test]
    fn duplicate_names_and_ids_are_rejected() {
        CounterSchema::new("Service", "worker", &[spec("Bytes", 0), spec("BYTES", 1)]).unwrap_err();
        CounterSchema::new("Service", "worker", &[spec("ı", 0), spec("I", 1)]).unwrap_err();
        CounterSchema::new("Service", "worker", &[spec("ẞς", 0), spec("ßσ", 1)]).unwrap_err();
        let mut schema = CounterSchema::new("Service", "worker", &[spec("Bytes", 0), spec("Requests", 1)]).unwrap();
        schema.counters[1].id = schema.counters[0].id;
        assert!(validate_collisions(&schema.counters).is_err());
    }

    #[test]
    fn empty_and_oversized_schemas_are_rejected() {
        CounterSchema::new("Service", "worker", &[]).unwrap_err();
        let specs = (0..=MAX_COUNTERS)
            .map(|index| spec(&format!("Counter {index}"), index))
            .collect::<Vec<_>>();
        CounterSchema::new("Service", "worker", &specs).unwrap_err();
    }

    #[test]
    fn binary_identity_must_be_a_filename() {
        let schema = CounterSchema::new("Service", "worker", &[spec("Bytes", 0)]).unwrap();
        for name in ["", "resources", "C:\\resources.dll", "../resources.dll", "bad\0.dll", "bad\".dll"] {
            assert!(schema.manifest(name).is_err(), "{name:?}");
        }
    }
}
