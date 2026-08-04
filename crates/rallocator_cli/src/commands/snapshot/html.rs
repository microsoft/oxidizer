use std::path::PathBuf;
use std::{fs, io};

use clap::Args;

#[derive(Args)]
pub(crate) struct VerbArgs {
    pub(crate) input: PathBuf,
    pub(crate) output: Option<PathBuf>,
}

pub(crate) fn verb(args: VerbArgs) -> Result<(), Error> {
    let bytes = fs::read(&args.input).map_err(Error::Io)?;
    let snapshot = rallocator_telemetry::decode(&bytes).map_err(Error::Decode)?;
    let output = args.output.unwrap_or_else(|| args.input.with_file_name("snapshot.html"));
    fs::write(&output, crate::report::render_html(&snapshot)).map_err(Error::Io)?;
    println!("{}", output.display());
    Ok(())
}

#[derive(Debug)]
pub(crate) enum Error {
    Io(io::Error),
    Decode(rallocator_telemetry::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "{error}"),
            Self::Decode(error) => write!(formatter, "invalid snapshot: {error}"),
        }
    }
}

impl std::error::Error for Error {}
