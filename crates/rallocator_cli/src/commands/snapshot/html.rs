// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::{fs, io};

use clap::Args;

#[derive(Args)]
pub(crate) struct VerbArgs {
    pub(crate) input: PathBuf,
    pub(crate) output: Option<PathBuf>,
}

pub(crate) fn verb(args: VerbArgs) -> Result<(), Error> {
    let explicit_output = args.output.is_some();
    let output = args.output.unwrap_or_else(|| args.input.with_extension("html"));
    if paths_refer_to_same_file(&args.input, &output).map_err(Error::Io)? {
        return Err(Error::SamePath(output));
    }
    if explicit_output && output.try_exists().map_err(Error::Io)? {
        return Err(Error::OutputExists(output));
    }

    let bytes = fs::read(&args.input).map_err(Error::Io)?;
    let snapshot = rallocator_telemetry::decode(&bytes).map_err(Error::Decode)?;
    let html = crate::report::render_html(&snapshot);
    if explicit_output {
        let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&output).map_err(|error| {
            if error.kind() == io::ErrorKind::AlreadyExists {
                Error::OutputExists(output.clone())
            } else {
                Error::Io(error)
            }
        })?;
        file.write_all(html.as_bytes()).map_err(Error::Io)?;
    } else {
        fs::write(&output, html).map_err(Error::Io)?;
    }
    println!("{}", output.display());
    Ok(())
}

fn paths_refer_to_same_file(input: &Path, output: &Path) -> io::Result<bool> {
    if input == output {
        return Ok(true);
    }
    let input = fs::canonicalize(input)?;
    match fs::canonicalize(output) {
        Ok(output) => Ok(input == output),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let parent = output
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."));
            match fs::canonicalize(parent) {
                Ok(parent) => Ok(output.file_name().is_some_and(|name| parent.join(name) == input)),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
                Err(error) => Err(error),
            }
        }
        Err(error) => Err(error),
    }
}

#[derive(Debug)]
pub(crate) enum Error {
    Io(io::Error),
    Decode(rallocator_telemetry::Error),
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
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    use rallocator_telemetry::snapshot::{Snapshot, Version};
    use rallocator_telemetry::{encode, encoded_len};

    use super::{Error, VerbArgs, verb};

    static NEXT_DIRECTORY: AtomicUsize = AtomicUsize::new(0);

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
    fn explicit_output_must_not_refer_to_input() {
        let directory = directory("same-explicit");
        fs::create_dir_all(&directory).unwrap();
        let input = directory.join("capture.rallocator");
        let output = directory.join(".").join("capture.rallocator");
        write_snapshot(&input);

        let result = verb(VerbArgs {
            input,
            output: Some(output.clone()),
        });

        assert!(matches!(result, Err(Error::SamePath(path)) if path == output));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn default_output_must_not_overwrite_input() {
        let directory = directory("same-default");
        fs::create_dir_all(&directory).unwrap();
        let input = directory.join("snapshot.html");
        write_snapshot(&input);

        let result = verb(VerbArgs {
            input: input.clone(),
            output: None,
        });

        assert!(matches!(result, Err(Error::SamePath(path)) if path == input));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn explicit_output_must_not_be_overwritten() {
        let directory = directory("existing-explicit");
        fs::create_dir_all(&directory).unwrap();
        let input = directory.join("capture.rallocator");
        let output = directory.join("report.html");
        write_snapshot(&input);
        fs::write(&output, "keep me").unwrap();

        let result = verb(VerbArgs {
            input,
            output: Some(output.clone()),
        });

        assert!(matches!(result, Err(Error::OutputExists(path)) if path == output));
        assert_eq!(fs::read_to_string(&output).unwrap(), "keep me");
        fs::remove_dir_all(directory).unwrap();
    }
}
