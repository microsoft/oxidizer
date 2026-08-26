// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Checks that the paths `docs/design.md` and `docs/requirements.md` name still exist.
//!
//! A design document goes stale where it restates something that nothing checks. Paths are the
//! worst of those: the documents live in `ohno_macros` while most of what they describe lives in
//! `ohno_macros_impl` and `ohno`, so a rename in one crate leaves a dangling reference in another
//! that no build ever touches. Three such references were found stale at once when the
//! implementation moved into `ohno_macros_impl`.
//!
//! The rule this enforces: **a path named in these documents is written from the workspace root**,
//! starting with `crates/`. A reference that cannot be written that way belongs in prose instead,
//! naming the role rather than the file, because prose that names a role does not rot when a file
//! moves.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The documents this checks, relative to this crate.
const DOCUMENTS: [&str; 2] = ["docs/design.md", "docs/requirements.md"];

/// The prefix a path reference carries to be resolvable from the workspace root.
const ROOT_PREFIX: &str = "crates/";

#[test]
fn every_path_named_in_the_documents_exists() {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace = crate_dir
        .parent()
        .and_then(Path::parent)
        .expect("this crate sits two directories below the workspace root");

    let mut missing = Vec::new();

    for document in DOCUMENTS {
        let path = crate_dir.join(document);
        let text = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{document} is readable: {error}"));

        for reference in path_references(&text) {
            if !workspace.join(&reference).exists() {
                missing.push(format!("{document}: `{reference}`"));
            }
        }
    }

    assert!(
        missing.is_empty(),
        "these documents name paths that do not exist:\n  {}\n\n\
         Either correct the path, or replace it with prose naming the role rather than the file.",
        missing.join("\n  ")
    );
}

/// Every distinct workspace-relative path the text names in a backticked span.
///
/// A span inside a fenced block is skipped: a fence holds an illustration, which is not a claim
/// about where something lives.
fn path_references(text: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut fenced = false;

    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }

        for span in backticked(line) {
            if let Some(reference) = resolvable(span) {
                _ = found.insert(reference);
            }
        }
    }

    found
}

/// The contents of each backtick-delimited span in one line.
///
/// Spans are taken between alternating backticks, so a line with an odd count leaves its last
/// opener unpaired and contributes nothing from it.
fn backticked(line: &str) -> Vec<&str> {
    let mut spans = Vec::new();
    let mut rest = line;

    while let Some(open) = rest.find('`') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('`') else { break };
        spans.push(&after[..close]);
        rest = &after[close + 1..];
    }

    spans
}

/// The part of `span` that names a path, if it names one at all.
///
/// A glob tail is cut back to the directory holding it, so `crates/ohno/tests/ui/*.stderr` asserts
/// that the fixture directory is there without asserting which fixtures are in it: the documents
/// speak about the set, not its members.
fn resolvable(span: &str) -> Option<String> {
    if !span.starts_with(ROOT_PREFIX) {
        return None;
    }

    let path = match span.find('*') {
        Some(star) => span[..star].rsplit_once('/').map_or("", |(directory, _)| directory),
        None => span,
    };

    let path = path.trim_end_matches('/');
    (!path.is_empty()).then(|| path.to_owned())
}
