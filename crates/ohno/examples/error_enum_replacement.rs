// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Replacing a `thiserror` error enum.
//!
//! Ohno derives on structs, not enums, so one variant per failure condition becomes one error type
//! per condition, behind a wrapper that lists them in `#[from(..)]` so `?` converts. The wrapper
//! declares no `#[display(..)]`, so it renders its cause verbatim and adds no text of its own.
//!
//! The condition types stay private and the wrapper answers `is_*()` questions about them, per
//! [M-ERRORS-CANONICAL-STRUCTS][guideline]. Callers never name a condition, so conditions can be
//! added or split without breaking them.
//!
//! Ported from [`cargo_metadata::Error`][upstream]. Note that `thiserror` interpolates the cause
//! into the message with `{0}` while ohno renders it on its own `caused by:` line, so drop the
//! trailing `: {0}` when moving a message over.
//!
//! [guideline]: https://microsoft.github.io/rust-guidelines/guidelines/libs/ux/index.html#M-ERRORS-CANONICAL-STRUCTS
//! [upstream]: https://docs.rs/cargo_metadata/0.23.1/src/cargo_metadata/errors.rs.html#25-52

use std::error::Error as StdError;
use std::fmt;

fn main() {
    for error in [
        metadata::run(),
        metadata::parse(b"\xff"),
        metadata::parse(b"no json here"),
        metadata::rejected("error: no such command: `metadata`"),
    ]
    .map(|result| result.expect_err("every stub path fails"))
    {
        // The wrapper contributes no message of its own, so this prints the condition's text.
        println!("{error}");

        // Callers ask the error what happened, instead of matching on an exposed enum.
        if error.is_start_failure() {
            println!("  -> could not start the process, retriable");
        } else if error.is_bad_output() {
            println!("  -> the process ran but its output was unusable");
        }
    }
}

/// Stands in for `serde_json::Error`, so this example needs no third-party dependency.
///
/// Any foreign error type behaves the same way: name it in `#[from(..)]` and it becomes a source.
#[derive(Debug)]
struct JsonSyntaxError;

impl fmt::Display for JsonSyntaxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("expected value at line 1 column 1")
    }
}

impl StdError for JsonSyntaxError {}

mod metadata {
    use std::error::Error as StdError;
    use std::str::{Utf8Error, from_utf8};
    use std::string::FromUtf8Error;

    use super::JsonSyntaxError;

    // One type per failure condition, each owning the message its enum variant used to carry.
    // They stay private: nothing outside this module names them.

    #[ohno::error]
    #[display("`cargo metadata` exited with an error: {stderr}")]
    struct CargoMetadataError {
        stderr: String,
    }

    #[ohno::error]
    #[from(std::io::Error)]
    #[display("failed to start `cargo metadata`")]
    struct StartError;

    #[ohno::error]
    #[from(Utf8Error)]
    #[display("cannot convert the stdout of `cargo metadata`")]
    struct StdoutError;

    #[ohno::error]
    #[from(FromUtf8Error)]
    #[display("cannot convert the stderr of `cargo metadata`")]
    struct StderrError;

    #[ohno::error]
    #[from(JsonSyntaxError)]
    #[display("failed to interpret `cargo metadata`'s json")]
    struct JsonError;

    #[ohno::error]
    #[display("could not find any json in the output of `cargo metadata`")]
    struct NoJsonError;

    /// The error this module returns.
    ///
    /// No `#[display(..)]`, so it renders its cause verbatim and stays invisible in the output.
    #[ohno::error]
    #[from(CargoMetadataError)]
    #[from(StartError)]
    #[from(StdoutError)]
    #[from(StderrError)]
    #[from(JsonError)]
    #[from(NoJsonError)]
    pub(crate) struct Error;

    impl Error {
        /// Returns `true` if `cargo metadata` could not be started at all.
        pub(crate) fn is_start_failure(&self) -> bool {
            // A failure condition is always the wrapper's immediate source.
            self.source().is_some_and(<dyn StdError>::is::<StartError>)
        }

        /// Returns `true` if the process ran but its output could not be used.
        pub(crate) fn is_bad_output(&self) -> bool {
            self.source().is_some_and(|source| {
                source.is::<StdoutError>() || source.is::<StderrError>() || source.is::<JsonError>() || source.is::<NoJsonError>()
            })
        }
    }

    /// Runs `cargo metadata` and returns its JSON output.
    pub(crate) fn run() -> Result<String, Error> {
        let stdout = start()?;
        parse(&stdout)
    }

    /// Extracts the JSON document out of already-captured output.
    pub(crate) fn parse(stdout: &[u8]) -> Result<String, Error> {
        let text = decode(stdout)?;
        Ok(locate_json(text)?.to_owned())
    }

    /// Reports the process having exited with a non-zero status.
    pub(crate) fn rejected(stderr: &str) -> Result<String, Error> {
        Err(CargoMetadataError::new(stderr).into())
    }

    fn start() -> Result<Vec<u8>, StartError> {
        // A real implementation would spawn the child process here.
        Err(std::io::Error::new(std::io::ErrorKind::NotFound, "no such file or directory").into())
    }

    fn decode(stdout: &[u8]) -> Result<&str, StdoutError> {
        Ok(from_utf8(stdout)?)
    }

    fn locate_json(text: &str) -> Result<&str, NoJsonError> {
        text.find('{').map(|start| &text[start..]).ok_or_else(NoJsonError::new)
    }
}

/*
Output:

failed to start `cargo metadata`
caused by: no such file or directory
  -> could not start the process, retriable
cannot convert the stdout of `cargo metadata`
caused by: invalid utf-8 sequence of 1 bytes from index 0
  -> the process ran but its output was unusable
could not find any json in the output of `cargo metadata`
  -> the process ran but its output was unusable
`cargo metadata` exited with an error: error: no such command: `metadata`
*/
