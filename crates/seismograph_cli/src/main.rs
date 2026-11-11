// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Command-line entry point for the Seismograph tools.

fn main() -> std::process::ExitCode {
    seismograph_cli::execute(std::env::args_os().collect())
}
