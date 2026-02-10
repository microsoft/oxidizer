#!/usr/bin/env -S cargo +nightly -Zscript
---
# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

[package]
edition = "2024"

[dependencies]
argh = "0.1"
ohno = { path = "../crates/ohno", features = ["app-err"] }
---

use std::fs;
use std::path::Path;
use std::process::{Command, ExitCode};

use ohno::{AppError, app_err, bail};

/// Create a GitHub release for a crate tag.
///
/// Parses a tag of the form `<crate>-v<major>.<minor>.<patch>`, extracts
/// the matching changelog section, and calls `gh release create`.
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
    let crate_path = validate_crate(crate_name)?;
    let body = extract_changelog(&crate_path, crate_name, version)?;
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
    if parts.len() != 3 || !parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())) {
        bail!("Tag '{tag}' does not contain a valid semver version");
    }

    Ok((crate_name, version))
}

/// Validates the crate name (no path traversal) and checks the directory exists.
fn validate_crate(crate_name: &str) -> Result<String, AppError> {
    if crate_name.contains("..") || crate_name.contains('/') || crate_name.contains('\\') {
        bail!("Invalid crate name: '{crate_name}'");
    }

    let crate_path = format!("crates/{crate_name}");
    if !Path::new(&crate_path).is_dir() {
        bail!("Crate directory not found: '{crate_path}'");
    }

    Ok(crate_path)
}

/// Reads the crate's CHANGELOG.md and extracts the section for the given version.
///
/// Looks for a `## [<version>]` or `## <version>` header and captures everything
/// until the next `## ` header or end of file.
#[ohno::enrich_err("failed to extract changelog for {crate_name} v{version}")]
fn extract_changelog(crate_path: &str, crate_name: &str, version: &str) -> Result<String, AppError> {
    let changelog_file = format!("{crate_path}/CHANGELOG.md");
    let default_body = format!("Release {crate_name} v{version}");

    let content = match fs::read_to_string(&changelog_file) {
        Ok(c) => c,
        Err(_) => {
            eprintln!("::warning::No CHANGELOG.md found at '{changelog_file}'. Using default release notes.");
            return Ok(default_body);
        }
    };

    let mut found = false;
    let mut body = String::new();

    for line in content.lines() {
        if line.starts_with("## ") {
            if found {
                // Reached the next section — stop.
                break;
            }
            // Match "## [version]" or "## version" with an optional trailing date/suffix.
            let header = line.trim_start_matches("## ").trim_start_matches('[');
            if header.starts_with(version)
                && header[version.len()..]
                    .starts_with(|c: char| c == ']' || c == ' ' || c == '\t')
                || header == version
            {
                found = true;
                continue;
            }
        } else if found {
            body.push_str(line);
            body.push('\n');
        }
    }

    let body = body.trim().to_string();
    if body.is_empty() {
        eprintln!("::warning::No changelog entry found for version {version}.");
        return Ok(default_body);
    }

    Ok(body)
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
