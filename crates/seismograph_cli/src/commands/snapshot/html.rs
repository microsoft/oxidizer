// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::{fs, io};

use clap::Args;

use crate::allocator_view::{Snapshot, Version};

#[derive(Args)]
pub(crate) struct VerbArgs {
    /// Replace an existing output file.
    #[arg(long)]
    pub(crate) force: bool,
    pub(crate) input: PathBuf,
    pub(crate) output: Option<PathBuf>,
}

pub(crate) fn verb(args: VerbArgs) -> Result<(), Error> {
    let output = args.output.unwrap_or_else(|| args.input.with_extension("html"));
    if paths_refer_to_same_file(&args.input, &output).map_err(Error::Io)? {
        return Err(Error::SamePath(output));
    }
    if !args.force && output.try_exists().map_err(Error::Io)? {
        return Err(Error::OutputExists(output));
    }

    let bytes = fs::read(&args.input).map_err(Error::Io)?;
    let (snapshot, sources) = decode_snapshot(&bytes)?;
    let html = crate::report::render_html_with_sources(&snapshot, &sources);
    if args.force {
        fs::write(&output, html).map_err(Error::Io)?;
    } else {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output)
            .map_err(|error| map_create_error(error, &output))?;
        file.write_all(html.as_bytes()).map_err(Error::Io)?;
    }

    println!("{}", output.display());
    Ok(())
}

fn decode_snapshot(bytes: &[u8]) -> Result<(Snapshot, Vec<seismograph::snapshot::SourceSnapshot>), Error> {
    let seismograph = match seismograph::snapshot::decode(bytes) {
        Ok(snapshot) => snapshot,
        Err(seismograph_error) => {
            return seismograph_rallocator::decode(bytes)
                .map(|native| {
                    let mut snapshot = empty_allocator_snapshot();
                    snapshot.native = Some(std::sync::Arc::new(native));
                    (snapshot, Vec::new())
                })
                .map_err(|native_error| Error::DecodeContainer {
                    seismograph: seismograph_error,
                    native: native_error,
                });
        }
    };

    let mut snapshot = empty_allocator_snapshot();
    if let Some(source) = allocator_source(&seismograph.sources)? {
        if source.schema_version != seismograph_rallocator::source::SCHEMA_VERSION {
            return Err(Error::UnsupportedSchema(source.schema_version));
        }
        snapshot.native = Some(std::sync::Arc::new(
            seismograph_rallocator::decode(&source.data).map_err(Error::DecodeAllocator)?,
        ));
    }
    snapshot.callers = Some(seismograph_rallocator::events::callers(&seismograph.events));
    if let Some(source) = seismograph
        .sources
        .iter()
        .find(|source| source.id == seismograph_runtime::snapshot::source::ID)
    {
        let runtime = seismograph_runtime::snapshot::decode(&source.data).map_err(Error::DecodeRuntime)?;
        snapshot.addresses = runtime
            .addresses
            .into_iter()
            .map(|lookup| {
                seismograph_rallocator::callers::AddressLookup::from_fields(seismograph_rallocator::callers::AddressLookupFields {
                    address: lookup.address,
                    symbol: lookup.symbol,
                    filename: lookup.filename,
                    line: lookup.line,
                    column: lookup.column,
                })
            })
            .collect();
    }
    if contains_runtime_events(&seismograph.events) {
        snapshot.runtime_events = Some(seismograph.events);
    }
    snapshot.metadata.capture_duration_nanos = seismograph.capture_duration_nanos;
    Ok((snapshot, seismograph.sources))
}

fn allocator_source(sources: &[seismograph::snapshot::SourceSnapshot]) -> Result<Option<&seismograph::snapshot::SourceSnapshot>, Error> {
    let mut matching = sources.iter().filter(|source| source.id == seismograph_rallocator::source::ID);
    let source = matching.next();
    if matching.next().is_some() {
        return Err(Error::DuplicateAllocatorSource);
    }
    Ok(source)
}

fn contains_runtime_events(events: &seismograph::recorder::event::Events) -> bool {
    !(events.threads.is_empty() && events.events.is_empty())
}

fn empty_allocator_snapshot() -> Snapshot {
    Snapshot::event_only(Version::new(0, 1, 0))
}

fn paths_refer_to_same_file(input: &Path, output: &Path) -> io::Result<bool> {
    if input == output {
        return Ok(true);
    }
    match same_file::is_same_file(input, output) {
        Ok(same) => Ok(same),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn map_create_error(error: io::Error, output: &Path) -> Error {
    if error.kind() == io::ErrorKind::AlreadyExists {
        Error::OutputExists(output.to_owned())
    } else {
        Error::Io(error)
    }
}

#[derive(Debug)]
pub(crate) enum Error {
    Io(io::Error),
    DecodeContainer {
        seismograph: seismograph::Error,
        native: seismograph_rallocator::Error,
    },
    DecodeAllocator(seismograph_rallocator::Error),
    DecodeRuntime(seismograph_runtime::snapshot::Error),
    UnsupportedSchema(u16),
    DuplicateAllocatorSource,
    SamePath(PathBuf),
    OutputExists(PathBuf),
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "{error}"),
            Self::DecodeContainer { seismograph, native } => {
                write!(
                    formatter,
                    "invalid snapshot: {seismograph}; native allocator snapshot decode also failed: {native}"
                )
            }
            Self::DecodeAllocator(error) => write!(formatter, "invalid allocator snapshot: {error}"),
            Self::DecodeRuntime(error) => write!(formatter, "invalid runtime snapshot: {error}"),
            Self::UnsupportedSchema(schema) => write!(formatter, "unsupported allocator source schema {schema}"),
            Self::DuplicateAllocatorSource => write!(formatter, "multiple native allocator sources make the inventory ambiguous"),
            Self::SamePath(path) => write!(formatter, "input and output refer to the same path: {}", path.display()),
            Self::OutputExists(path) => write!(formatter, "refusing to overwrite existing output: {}", path.display()),
        }
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::{fs, io};

    use seismograph_rallocator::{encode, encoded_len};

    use super::{
        Error, VerbArgs, allocator_source, contains_runtime_events, decode_snapshot, empty_allocator_snapshot, map_create_error,
        paths_refer_to_same_file, verb,
    };
    use crate::allocator_view::{SkippedSection, SkippedSectionFields, Snapshot, Version};

    static NEXT_DIRECTORY: AtomicUsize = AtomicUsize::new(0);
    static ALLOCATOR_SOURCE: seismograph::snapshot::Source =
        seismograph::snapshot::Source::new(seismograph_rallocator::source::ID, "test-rallocator", 3, capture_allocator_source);

    fn run_source_test_in_child(name: &str) -> bool {
        let name = format!("commands::snapshot::html::tests::{name}");
        if std::env::var("SEISMOGRAPH_HTML_SOURCE_TEST").as_deref() == Ok(name.as_str()) {
            return false;
        }
        // Source registration is process-global and permanent; cargo test shares
        // a process, unlike nextest. Give each conflicting source its own registry.
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--nocapture"])
            .env("SEISMOGRAPH_HTML_SOURCE_TEST", &name)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        true
    }

    fn capture_allocator_source(
        _context: seismograph::snapshot::SnapshotContext<'_>,
    ) -> Result<seismograph::snapshot::SourceData, seismograph::Error> {
        let snapshot = seismograph_rallocator::native::Snapshot::default();
        let mut data = seismograph::snapshot::SourceData::zeroed(encoded_len(&snapshot).unwrap())?;
        encode(&snapshot, data.as_mut_bytes()).unwrap();
        Ok(data)
    }

    fn directory(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join(format!(
            "html-unit-{name}-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn write_snapshot(path: &Path) {
        let snapshot = seismograph_rallocator::native::Snapshot::default();
        let mut bytes = vec![0; encoded_len(&snapshot).unwrap()];
        encode(&snapshot, &mut bytes).unwrap();
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn invalid_container_preserves_both_decode_errors() {
        assert!(matches!(decode_snapshot(b"invalid"), Err(Error::DecodeContainer { .. })));
    }

    #[test]
    fn allocator_decode_errors_have_specific_messages() {
        let error = seismograph_rallocator::decode(b"invalid").unwrap_err();
        assert_eq!(
            Error::DecodeAllocator(error).to_string(),
            format!("invalid allocator snapshot: {error}")
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "filesystem path identity is exercised by native tests")]
    fn explicit_output_must_not_refer_to_input() {
        let directory = directory("same-explicit");
        fs::create_dir_all(&directory).unwrap();
        let input = directory.join("capture.rallocator");
        let output = directory.join(".").join("capture.rallocator");
        write_snapshot(&input);

        let result = verb(VerbArgs {
            force: false,
            input,
            output: Some(output.clone()),
        });

        assert!(matches!(result, Err(Error::SamePath(path)) if path == output));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    #[cfg_attr(miri, ignore = "filesystem path identity is exercised by native tests")]
    fn default_output_must_not_overwrite_input() {
        let directory = directory("same-default");
        fs::create_dir_all(&directory).unwrap();
        let input = directory.join("snapshot.html");
        write_snapshot(&input);

        let result = verb(VerbArgs {
            force: false,
            input: input.clone(),
            output: None,
        });

        assert!(matches!(result, Err(Error::SamePath(path)) if path == input));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    #[cfg_attr(miri, ignore = "filesystem overwrite protection is exercised by native tests")]
    fn explicit_output_must_not_be_overwritten() {
        let directory = directory("existing-explicit");
        fs::create_dir_all(&directory).unwrap();
        let input = directory.join("capture.rallocator");
        let output = directory.join("report.html");
        write_snapshot(&input);
        fs::write(&output, "keep me").unwrap();

        let result = verb(VerbArgs {
            force: false,
            input,
            output: Some(output.clone()),
        });

        assert!(matches!(result, Err(Error::OutputExists(path)) if path == output));
        assert_eq!(fs::read_to_string(&output).unwrap(), "keep me");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    #[cfg_attr(miri, ignore = "filesystem overwrite protection is exercised by native tests")]
    fn default_output_must_not_be_overwritten() {
        let directory = directory("existing-default");
        fs::create_dir_all(&directory).unwrap();
        let input = directory.join("capture.rallocator");
        let output = directory.join("capture.html");
        write_snapshot(&input);
        fs::write(&output, "keep me").unwrap();

        let result = verb(VerbArgs {
            force: false,
            input,
            output: None,
        });

        assert!(matches!(result, Err(Error::OutputExists(path)) if path == output));
        assert_eq!(fs::read_to_string(&output).unwrap(), "keep me");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    #[cfg_attr(miri, ignore = "filesystem replacement is exercised by native tests")]
    fn force_replaces_existing_output() {
        let directory = directory("force");
        fs::create_dir_all(&directory).unwrap();
        let input = directory.join("capture.rallocator");
        let output = directory.join("report.html");
        write_snapshot(&input);
        fs::write(&output, "replace me").unwrap();

        verb(VerbArgs {
            force: true,
            input,
            output: Some(output.clone()),
        })
        .unwrap();

        assert!(fs::read_to_string(&output).unwrap().contains("<style>"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    #[cfg_attr(miri, ignore = "filesystem hard-link identity is exercised by native tests")]
    fn force_must_not_overwrite_input_through_hard_link() {
        let directory = directory("hard-link");
        fs::create_dir_all(&directory).unwrap();
        let input = directory.join("capture.rallocator");
        let output = directory.join("report.html");
        write_snapshot(&input);
        fs::hard_link(&input, &output).unwrap();
        let original = fs::read(&input).unwrap();

        let result = verb(VerbArgs {
            force: true,
            input: input.clone(),
            output: Some(output.clone()),
        });

        assert!(matches!(result, Err(Error::SamePath(path)) if path == output));
        assert_eq!(fs::read(input).unwrap(), original);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn create_errors_are_classified() {
        let output = Path::new("report.html");
        let error = map_create_error(io::Error::from(io::ErrorKind::AlreadyExists), output);
        assert!(matches!(error, Error::OutputExists(path) if path == output));

        let error = map_create_error(io::Error::from(io::ErrorKind::PermissionDenied), output);
        assert!(matches!(error, Error::Io(error) if error.kind() == io::ErrorKind::PermissionDenied));
    }

    #[test]
    #[cfg_attr(miri, ignore = "filesystem error propagation is exercised by native tests")]
    fn same_file_errors_are_propagated() {
        let error = paths_refer_to_same_file(Path::new("\0"), Path::new("report.html")).unwrap_err();
        assert_ne!(error.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn same_path_error_includes_the_path() {
        let path = PathBuf::from("capture.rallocator");
        assert_eq!(
            Error::SamePath(path).to_string(),
            "input and output refer to the same path: capture.rallocator"
        );
    }

    #[test]
    fn skipped_sections_render_compatibility_details() {
        let mut snapshot = Snapshot::new(Version::new(0, 1, 0));
        snapshot
            .skipped_sections
            .push(SkippedSection::from_fields(SkippedSectionFields { id: 999, version: 0 }));
        snapshot
            .skipped_sections
            .push(SkippedSection::from_fields(SkippedSectionFields { id: 1000, version: 1 }));

        let html = crate::report::render_html(&snapshot);

        assert!(html.contains("999 (version 0), 1000 (version 1)"));
        assert!(html.contains("unknown identifiers or versions unsupported by this decoder"));
        assert!(html.contains("compatible seismograph version"));
    }

    #[test]
    fn native_snapshot_without_allocator_source_marks_memory_unavailable() {
        let snapshot = empty_allocator_snapshot();
        assert_eq!(snapshot.metadata.telemetry_schema_version, 3);
        assert!(!snapshot.allocator_state_available);
    }

    #[test]
    fn native_snapshot_allocator_source_is_decoded() {
        if run_source_test_in_child("native_snapshot_allocator_source_is_decoded") {
            return;
        }
        seismograph::snapshot::register_source(&ALLOCATOR_SOURCE);
        let bytes = seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default())
            .unwrap()
            .as_bytes()
            .to_vec();

        let (snapshot, sources) = super::decode_snapshot(&bytes).unwrap();

        assert_eq!(
            (
                snapshot.metadata.telemetry_schema_version,
                sources.iter().any(|source| source.id == seismograph_rallocator::source::ID),
            ),
            (3, true)
        );
    }

    #[test]
    fn native_source_rejects_unsupported_schema_before_payload_decode() {
        static SOURCE: seismograph::snapshot::Source = seismograph::snapshot::Source::new(
            seismograph_rallocator::source::ID,
            "unsupported-native-schema",
            2,
            capture_allocator_source,
        );
        if run_source_test_in_child("native_source_rejects_unsupported_schema_before_payload_decode") {
            return;
        }
        seismograph::snapshot::register_source(&SOURCE);
        let recording = seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default()).unwrap();
        let error = decode_snapshot(recording.as_bytes()).unwrap_err();
        assert!(matches!(error, Error::UnsupportedSchema(2)));
        assert_eq!(error.to_string(), "unsupported allocator source schema 2");
    }

    #[test]
    fn corrupt_native_source_payload_preserves_allocator_error() {
        static SOURCE: seismograph::snapshot::Source = seismograph::snapshot::Source::new(
            seismograph_rallocator::source::ID,
            "corrupt-native",
            seismograph_rallocator::source::SCHEMA_VERSION,
            capture_corrupt_source,
        );
        if run_source_test_in_child("corrupt_native_source_payload_preserves_allocator_error") {
            return;
        }
        seismograph::snapshot::register_source(&SOURCE);
        let recording = seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default()).unwrap();
        let error = decode_snapshot(recording.as_bytes()).unwrap_err();
        assert!(matches!(error, Error::DecodeAllocator(_)));
        assert!(error.to_string().starts_with("invalid allocator snapshot: "));
        assert_eq!(
            Error::DuplicateAllocatorSource.to_string(),
            "multiple native allocator sources make the inventory ambiguous"
        );
    }

    #[test]
    fn corrupt_runtime_source_payload_preserves_runtime_error() {
        static SOURCE: seismograph::snapshot::Source = seismograph::snapshot::Source::new(
            seismograph_runtime::snapshot::source::ID,
            "corrupt-runtime",
            6,
            capture_corrupt_source,
        );
        if run_source_test_in_child("corrupt_runtime_source_payload_preserves_runtime_error") {
            return;
        }
        seismograph::snapshot::register_source(&SOURCE);
        let recording = seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default()).unwrap();
        let error = decode_snapshot(recording.as_bytes()).unwrap_err();
        assert!(matches!(error, Error::DecodeRuntime(_)));
        assert!(error.to_string().starts_with("invalid runtime snapshot: "));
    }

    fn capture_corrupt_source(
        _context: seismograph::snapshot::SnapshotContext<'_>,
    ) -> Result<seismograph::snapshot::SourceData, seismograph::Error> {
        seismograph::snapshot::SourceData::zeroed(8)
    }

    #[test]
    fn runtime_source_projects_symbol_names_and_locations_into_report() {
        static SOURCE: seismograph::snapshot::Source = seismograph::snapshot::Source::new(
            seismograph_runtime::snapshot::source::ID,
            "runtime-symbol",
            6,
            capture_runtime_symbol,
        );
        if run_source_test_in_child("runtime_source_projects_symbol_names_and_locations_into_report") {
            return;
        }
        seismograph::snapshot::register_source(&SOURCE);
        let recording = seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default()).unwrap();
        let (snapshot, sources) = decode_snapshot(recording.as_bytes()).unwrap();
        let fields = &snapshot.addresses[0];
        assert_eq!(fields.address, 0x1234);
        assert_eq!(fields.symbol.as_deref(), Some("task::poll"));
        assert_eq!(fields.filename.as_deref(), Some("worker.rs"));
        assert_eq!((fields.line, fields.column), (Some(42), Some(7)));
        assert!(sources.iter().any(|source| source.id == seismograph_runtime::snapshot::source::ID));
    }

    fn capture_runtime_symbol(
        _context: seismograph::snapshot::SnapshotContext<'_>,
    ) -> Result<seismograph::snapshot::SourceData, seismograph::Error> {
        // Schema 6, no runtimes, one address lookup with symbol and source location.
        let mut bytes = b"SEISRUNT".to_vec();
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&6_u16.to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u32.to_le_bytes());
        bytes.extend_from_slice(&0x1234_u64.to_le_bytes());
        bytes.extend_from_slice(&10_u32.to_le_bytes());
        bytes.extend_from_slice(&9_u32.to_le_bytes());
        bytes.extend_from_slice(&42_u32.to_le_bytes());
        bytes.extend_from_slice(&7_u32.to_le_bytes());
        bytes.extend_from_slice(b"task::pollworker.rs");
        let mut data = seismograph::snapshot::SourceData::zeroed(bytes.len())?;
        data.as_mut_bytes().copy_from_slice(&bytes);
        Ok(data)
    }

    #[test]
    fn native_source_and_event_detection_require_the_expected_content() {
        let allocator = seismograph::snapshot::SourceSnapshot {
            id: seismograph_rallocator::source::ID,
            name: "allocator".into(),
            schema_version: 1,
            data: Vec::new(),
        };
        let other = seismograph::snapshot::SourceSnapshot {
            id: seismograph::snapshot::SourceId::new(999),
            name: "other".into(),
            schema_version: 1,
            data: Vec::new(),
        };
        let empty = seismograph::recorder::event::Events::default();
        let mut threads = empty.clone();
        threads.threads.push(seismograph::recorder::thread::ThreadLog {
            thread_id: seismograph::recorder::thread::ThreadId::new(1),
            total_events: 0,
            lost_events: 0,
            name: "worker".into(),
        });
        let mut events = empty.clone();
        events.events.push(seismograph::recorder::event::Event {
            thread_id: seismograph::recorder::thread::ThreadId::new(1),
            sequence: seismograph::recorder::event::EventSequence::new(1),
            timestamp: seismograph::recorder::event::EventTimestamp::from_ticks(1),
            kind: seismograph::recorder::event::EventKind::ArcClone,
            payload: seismograph::recorder::event::EventPayload::Object(seismograph::recorder::event::ObjectId::new(1)),
            call_stack: Vec::new(),
        });

        assert_eq!(
            (
                allocator_source(std::slice::from_ref(&allocator))
                    .unwrap()
                    .map(|source| source.name.as_str()),
                allocator_source(std::slice::from_ref(&other))
                    .unwrap()
                    .map(|source| source.name.as_str()),
                contains_runtime_events(&empty),
                contains_runtime_events(&threads),
                contains_runtime_events(&events),
            ),
            (Some("allocator"), None, false, true, true)
        );
    }

    #[test]
    fn duplicate_native_sources_are_rejected_instead_of_selecting_one() {
        let source = seismograph::snapshot::SourceSnapshot {
            id: seismograph_rallocator::source::ID,
            name: "native".into(),
            schema_version: 3,
            data: Vec::new(),
        };
        assert!(matches!(
            allocator_source(&[source.clone(), source]),
            Err(Error::DuplicateAllocatorSource)
        ));
    }
}
