#!/usr/bin/env -S cargo +nightly -Zscript
---
# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
#
# This script is invoked by the `create-release` GitHub Actions workflow
# when a version tag is pushed. It is not intended to be run manually.

[package]
edition = "2024"

[dependencies]
argh = "0.1"
ohno = { path = "../crates/ohno", features = ["app-err"] }
---

use std::path::Path;
use std::process::{Command, ExitCode};

use ohno::{AppError, app_err, bail};

/// Create a GitHub release for a crate tag.
///
/// Parses a tag of the form `<crate>-v<major>.<minor>.<patch>` and calls
/// `gh release create`.
#[derive(argh::FromArgs)]
struct Args {
    /// the git tag name (e.g. "bytesbuf-v1.2.3")
    #[argh(positional)]
    tag: String,

    /// the GitHub repository in "owner/name" format (e.g. "microsoft/oxidizer")
    #[argh(option)]
    repo: String,
}

fn main() -> ExitCode {
    let args: Args = argh::from_env();

    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("::error::{e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &Args) -> Result<(), AppError> {
    let (crate_name, version) = parse_tag(&args.tag)?;
    validate_crate(crate_name)?;
    let body = format!("Release {crate_name} v{version}");
    println!("Creating release for {crate_name} v{version} with body:\n{body}");
    create_release(&args.tag, &args.repo, crate_name, version, &body)
}

/// Splits a tag like `foo-v1.2.3` into `("foo", "1.2.3")`.
fn parse_tag(tag: &str) -> Result<(&str, &str), AppError> {
    let idx = tag
        .rfind("-v")
        .ok_or_else(|| app_err!("Tag '{tag}' does not match <crate>-v<version>"))?;

    let crate_name = &tag[..idx];
    let version = &tag[idx + 2..]; // skip "-v"

    if crate_name.is_empty() {
        bail!("Tag '{tag}' has an empty crate name");
    }

    // Validate version looks like X.Y.Z
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() != 3
        || !parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
    {
        bail!("Tag '{tag}' does not contain a valid semver version");
    }

    Ok((crate_name, version))
}

/// Validates the crate name and checks that its directory exists.
fn validate_crate(crate_name: &str) -> Result<(), AppError> {
    if crate_name.contains("..") || crate_name.contains('/') || crate_name.contains('\\') {
        bail!("Invalid crate name: '{crate_name}'");
    }

    let crate_path = format!("crates/{crate_name}");
    if !Path::new(&crate_path).is_dir() {
        bail!("Crate directory not found: '{crate_path}'");
    }

    Ok(())
}

/// Invokes `gh release create` to publish the release.
#[ohno::enrich_err("failed to run `gh`")]
fn create_release(
    tag: &str,
    repo: &str,
    crate_name: &str,
    version: &str,
    body: &str,
) -> Result<(), AppError> {
    let title = format!("{crate_name} v{version}");

    let status = Command::new("gh")
        .args([
            "release",
            "create",
            tag,
            "--repo",
            repo,
            "--title",
            &title,
            "--notes",
            body,
        ])
        .status()?;

    if !status.success() {
        bail!("'gh release create' exited with {status}");
    }

    Ok(())
}
