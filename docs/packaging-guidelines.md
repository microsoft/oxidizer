# Packaging Guidelines

This document describes what gets published to crates.io for each crate in this
workspace, and the rules you must follow when adding files so that published
`.crate` tarballs stay correct and reproducible.

## What gets published

Published crate contents are controlled by an explicit **`include` allowlist**
defined once in the root `Cargo.toml` under `[workspace.package]`. Every crate
opts in with `include = { workspace = true }` (or `include.workspace = true`).

Only these paths are packaged:

```toml
include = [
    "/src/**",
    "/build.rs",
    "/tests/**",
    "!/tests/__fuzz__/**",
    "/docs/**/*.md",
    "/Cargo.toml",
    "/README.md",
    "/LICENSE*",
]
```

Anything not listed (for example `examples/`, `benches/`, `logo.png`,
`favicon.ico`, `AGENTS.md`, `CHANGELOG.md`, `docs/diagrams/*`, editable
`*.graphml` sources, internal design notes) is **not** shipped.

## What belongs in the package

The package should contain what a **dependent** needs: everything required to
build the library, plus the metadata crates.io renders.

- **Build-required inputs ship.** `src/**`, `build.rs`, plus any file compiled
  in via `include_str!`/`include_bytes!` (see `docs/**/*.md` below).
- **Key metadata ships.** `Cargo.toml`, `README.md` (rendered on crates.io),
  and `LICENSE*`.
- **Tests ship as source.** They are cheap to include in the tarball. They do
  not guarantee a packaged crate is verifiable on its own: Cargo strips
  path-only dev-dependencies from the published manifest, so any test that
  needs an unpublished in-workspace helper (for example `testing_aids`, or a
  `private-test-util` feature reached only through a path dev-dependency) will
  not build from crates.io alone. Fuzz targets are excluded because they need a
  separate toolchain to build at all.
- **Examples and benchmarks do not ship.** They are development code. No
  dependent builds them, they routinely depend on unpublished in-workspace
  fixture crates, and shipping them enlarges the tarball and the LFS exposure
  surface for no consumer benefit.
- **Everything else is dropped.** `CHANGELOG.md` is intentionally excluded: it
  is neither build-required nor rendered by crates.io.

A crate that declares an explicit `[[example]]` or `[[bench]]` target therefore
publishes a manifest entry whose source is absent. Cargo does not build those
targets for a dependent, so this is inert.

## Why an allowlist instead of `exclude`

The allowlist exists primarily to keep **Git LFS-tracked binaries out of the
package**, which is a correctness requirement, not just tidiness.

`cargo` has no awareness of Git LFS. Depending on whether the machine that runs
`cargo package` has smudged LFS, an LFS-tracked file is packaged either as its
real bytes **or as a ~130-byte pointer stub**. Either way the result is
non-deterministic: the same crate version can produce two different `.crate`
checksums.

That breaks downstream builds. When crate `B` depends on crate `A`, `B`'s
embedded `Cargo.lock` records a checksum for `A`. If `A` was uploaded with
different bytes than the checksum `B` recorded, docs.rs fails `B` with:

```
error: checksum for `A vX.Y.Z` changed between lock files
```

This has taken down docs.rs builds for most of the workspace more than once. An
allowlist prevents it **by construction**: no LFS binary is ever in a packaged
path, so packaging is deterministic regardless of LFS state. An `exclude`
denylist would not be robust, because any newly added binary would leak in
until someone remembered to exclude it.

## Rules when adding files

1. **Never place a Git LFS-tracked file in a packaged path.** The LFS globs in
   `.gitattributes` (`*.png`, `*.ico`, `*.jpg`, `*.pdf`, `*.zip`, `*.dll`,
   `*.exe`, ...) must not appear under `src/`, `tests/`, or `docs/`. Reference
   such assets by **absolute URL** instead, e.g.:

   ```rust
   #![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/<crate>/logo.png")]
   ```

2. **Doc fragments compiled with `include_str!` must be Markdown under
   `docs/`.** The allowlist packages `docs/**/*.md` (Markdown only), so a
   fragment such as `#[doc = include_str!("../docs/snippets/foo.md")]` is
   shipped, while a binary diagram beside it under `docs/diagrams/` is not.
   Reference rendered diagrams by absolute URL, exactly like logos.

3. **Use `docs/` (plural), not `doc/`.** `doc` collides with rustdoc's
   `target/doc` output and is non-conventional.

4. **New crates inherit the policy automatically** as long as their
   `Cargo.toml` opts in with `include = { workspace = true }`. The crate
   template (`scripts/crate-template`) should keep this line.

## How to verify

List exactly what a crate would package and confirm no binary slips in:

```sh
cargo package -p <crate> --list
```

The output must not contain any `*.png`, `*.ico`, `*.jpg`, or other binary
asset. To prove a crate still builds from its packaged form (this is what
docs.rs effectively does), run a verifying package, which builds the extracted
tarball:

```sh
cargo package -p <crate>
```

If a required `include_str!` fragment was wrongly excluded, this step fails.
