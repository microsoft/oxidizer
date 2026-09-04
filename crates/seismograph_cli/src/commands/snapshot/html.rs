// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::{fs, io};

use clap::Args;
use seismograph_rallocator::snapshot::{Snapshot, Version};

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
    let Ok(seismograph) = seismograph::snapshot::decode(bytes) else {
        return seismograph_rallocator::decode(bytes)
            .map(|snapshot| (snapshot, Vec::new()))
            .map_err(Error::Decode);
    };

    let mut snapshot = match seismograph
        .sources
        .iter()
        .find(|source| source.id == seismograph_rallocator::source::ID)
    {
        Some(source) => seismograph_rallocator::decode(&source.data).map_err(Error::Decode)?,
        None => empty_allocator_snapshot(),
    };
    if !seismograph.events.threads.is_empty() || !seismograph.events.events.is_empty() {
        snapshot.runtime_events = Some(seismograph.events);
    }
    snapshot.metadata.capture_duration_nanos = seismograph.capture_duration_nanos;
    Ok((snapshot, seismograph.sources))
}

fn empty_allocator_snapshot() -> Snapshot {
    Snapshot::new(Version::new(0, 1, 0))
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
    Decode(seismograph_rallocator::Error),
    SamePath(PathBuf),
    OutputExists(PathBuf),
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "{error}"),
            Self::Decode(error) => write!(formatter, "invalid snapshot: {error}"),
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

    use seismograph_rallocator::snapshot::{SkippedSection, SkippedSectionFields, Snapshot, Version};
    use seismograph_rallocator::{encode, encoded_len};

    use super::{Error, VerbArgs, empty_allocator_snapshot, map_create_error, paths_refer_to_same_file, verb};

    static NEXT_DIRECTORY: AtomicUsize = AtomicUsize::new(0);
    static ALLOCATOR_SOURCE: seismograph::snapshot::Source =
        seismograph::snapshot::Source::new(seismograph_rallocator::source::ID, "test-rallocator", 1, capture_allocator_source);

    fn capture_allocator_source(
        _context: seismograph::snapshot::SnapshotContext<'_>,
    ) -> Result<seismograph::snapshot::SourceData, seismograph::Error> {
        let snapshot = Snapshot::new(Version::new(0, 1, 0));
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
        let snapshot = Snapshot::new(Version::new(0, 1, 0));
        let mut bytes = vec![0; encoded_len(&snapshot).unwrap()];
        encode(&snapshot, &mut bytes).unwrap();
        fs::write(path, bytes).unwrap();
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
    fn native_snapshot_without_allocator_source_uses_empty_memory_snapshot() {
        assert_eq!(empty_allocator_snapshot().metadata.telemetry_schema_version, 1);
    }

    #[test]
    fn native_snapshot_allocator_source_is_decoded() {
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
            (1, true)
        );
    }
}
