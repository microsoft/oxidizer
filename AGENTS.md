# AI Agents Guidelines

Code in this repository should follow the guidelines specified in the [Microsoft Rust Guidelines](https://microsoft.github.io/rust-guidelines/agents/all.txt).

## Documentation language

Use the project `simple-english` skill for crate documentation and other
human-facing documentation.

## README Files

Crate README files are auto-generated via `just anvil-readme --fix`. Do not manually update them. Because they are generated, do not edit them by hand and do not raise their formatting, wording, or casing (for example the title-cased crate name) as issues in code review - such content is not authored here and is regenerated from the crate's doc comments.

## Writing tests

Use the project `writing-tests` skill when you add or change tests.

## Executing `just` commands

Use each Anvil recipe's `--package PACKAGE_NAME` option when it supports package
scoping. The package name is the Cargo.toml `[package].name`, which may differ
from the directory name.

## Pre-commit Checklist

- Run `just anvil-clippy` to verify the code compiles without linter errors.
- Run `just anvil-fmt --fix` to format code.
- Run `just anvil-readme --fix` to regenerate crate-level README files.
- Run `just anvil-spellcheck` to check spelling in code comments and docs.

## Spelling

The spell checker dictionary is in the `.spelling` file, one word per line in arbitrary order.

## Changelogs

Ordinary code PRs do not update changelogs. Changelog entries are prepared as
part of an on-demand version PR through the `prepare-release` skill; see
[docs/releasing.md](docs/releasing.md).

## Releasing Packages

See [docs/releasing.md](docs/releasing.md) for the on-demand version-planning
workflow. Use the repository's `prepare-release` skill as the human entry point;
it coordinates `increment-versions` and release changelogs. Version preparation
is not required for ordinary code PRs and does not authorize merging,
tagging, or publication.

## Packaging

What ships in each published `.crate` is controlled by an explicit `include`
allowlist in `[workspace.package]` (each crate opts in via
`include = { workspace = true }`). The key rule: **never place a Git LFS-tracked
binary (logos, diagrams, etc.) in a packaged path** — it makes the package
non-reproducible and breaks docs.rs. Reference such assets by absolute URL
instead. See [docs/packaging-guidelines.md](docs/packaging-guidelines.md) for
the full policy, the LFS pitfall it prevents, and how to verify a crate's
packaged contents.

## Pull Requests

Pull request titles must follow [Conventional Commits](https://www.conventionalcommits.org/) naming, e.g. `feat(bytesbuf): add new metric` or `fix(cachet): correct eviction logic`.

## Feature-gated Doctests

Doctests that reference items behind a Cargo feature must compile both with and without that feature; wrap their bodies in hidden `#[cfg(...)]` shims. See [docs/feature-gated-doctests.md](docs/feature-gated-doctests.md).

## Optional Dependencies in Test Builds

Feature-dependent code is gated behind `cfg(any(test, feature = "foo"))` so that a crate's test build compiles it without enumerating features. Features must therefore be additive. Because `cfg(test)` does not activate Cargo features, every optional dependency must also be declared as a non-optional dev-dependency, carrying whatever dependency features the feature activates. See [docs/optional-deps-in-test-builds.md](docs/optional-deps-in-test-builds.md).

## Publishing Private Test, Bench and Example Utils

Shared fixtures that a crate's own tests, benchmarks and examples need - mock servers, scripted backends - must not become public API. When the fixtures themselves depend on the crate under test, they also must not live in a package that depends on that crate, because that is a dependency cycle; host them in the crate's implementation package behind a `private-test-util` feature that the public facade enables through a dev-dependency. When the fixtures do not need the crate under test, a `publish = false` fixture package remains the simpler answer.

This pattern is an exception to "Optional Dependencies in Test Builds" above: the fixture module is gated on `feature = "private-test-util"` alone (not `cfg(any(test, feature = ...))`), and its optional dependencies are not mirrored as non-optional dev-dependencies of the implementation crate, because the implementation crate's own unit tests do not consume the fixtures. See [docs/private-test-utils.md](docs/private-test-utils.md).

## `no_std` Support

`no_std` support is optional when deciding whether to adopt or expand it. Support for constrained targets must not justify disproportionate implementation complexity, such as extensive `cfg` branching or specialized fallbacks for platforms without pointer-width atomics.

Once a crate documents a `no_std` configuration as supported, that configuration is a real compatibility promise, not best-effort support. It must work correctly, be tested in CI, and be documented with its actual prerequisites and support boundary, including requirements such as `alloc`, pointer-width atomics, or specific target capabilities.

### `no_std`-only code is exempt from coverage and mutation testing

Code selected by the *absence* of `std` — `#[cfg(not(feature = "std"))]` and equivalents — is excluded from the coverage gate and from mutation testing. Neither harness is guaranteed to build a `no_std` configuration: mutation testing runs with default features, and in the coverage run's `--no-default-features` leg Cargo feature unification across the selected dependency graph can re-enable `std` anyway. Such code is therefore often absent from the measured build entirely, and holding it to a coverage or mutation obligation measures the harness rather than the code.

Mark it with both exclusions:

```rust
#[cfg(not(feature = "std"))]
#[cfg_attr(coverage_nightly, coverage(off))] // no_std-only path (AGENTS.md, "no_std Support").
#[cfg_attr(test, mutants::skip)] // no_std-only path (AGENTS.md, "no_std Support").
fn capture() -> Self {
    Self::Disabled
}
```

`coverage(off)` needs `#![cfg_attr(coverage_nightly, feature(coverage_attribute))]` in the crate root.

Attach the exclusions to the `no_std` arm alone, splitting the item into per-configuration definitions where the two arms share one function. The `std` arm keeps its full coverage and mutation obligations. The exemption covers only code that a `no_std` build selects *instead of* a `std` build; ungated code that merely also compiles under `no_std` is tested normally.

## GitHub Actions Pinning

Pin GitHub Actions to full-length commit SHAs with a version comment unless
the exact release is verified as a GitHub immutable release. For immutable
releases, use the exact version tag instead; never use moving major/minor aliases.

Verify each new release with
`gh api repos/OWNER/REPO/releases/tags/TAG --jq .immutable`; only `true` qualifies
for the tag exception.

Preserve pinning-rationale comments when updating dependencies or addressing
review feedback, and update their versioned links and references. Do not delete
the rationale merely because Dependabot proposes a SHA pin: retain the exact
version tag when the new release is still immutable.

## Required CI Checks

The generated Anvil workflow publishes the `Required Anvil checks` fan-in.
Repository-specific checks outside Anvil publish `Required repository checks`.
Treat both display names as ruleset interfaces and keep them stable.

## Maintainability

While it is fine to use `.expect()`, the precondition is that it is either a programming error (the caller did something wrong)
or a situation that can never happen (in the absence of bugs). The expect-message must document either what the caller did wrong
in their code or why we believe the situation could never happen.

This is bad code: `self_span.get(self_offset..).expect("self_offset out of bounds")` - it does not explain what the caller did
wrong and it does not explain why we believe this access can never be out of bounds.

This is good code: `self_span.get(self_offset..).expect("guarded by min() above to never exceed span length")` - this explains
why we believe the operation can never cause an out of bounds access.

In test code, use `.unwrap()` instead of `.expect()` because the backtrace will be informative enough already.

In example code, prefer `.expect()` unless it gets too verbose - `.unwrap()` is fine if you need to condense the text for readability.

# [Testing tracing events](docs/tracing-tests.md)

How to test `tracing` output without cross-test pollution. In packages that declare
a direct normal or dev-dependency on `tracing`, every Cargo unit-test and
integration-test executable must invoke `testing_aids::init_tracing!()` before tests run.
Also initialize specific targets that explicitly test or capture tracing through a
dependency. Build-only and procedural-macro host dependencies do not qualify a consumer.
Apply the rule across declared features and platforms. Transitive presence alone
does not qualify. Each integration target needs its own initializer.
The guide defines isolated exceptions for disabled tracing and custom subscribers.

**Open this when**: writing or moving any test that inspects `tracing` output;
adding a log/event emission that needs coverage; adding a test target or changing
dependencies that can introduce tracing; tempted to install a global subscriber
in a unit test.

# [Benchmarks](docs/benchmarks.md)

Criterion benchmark design (single-threaded by default, elementary operations,
`black_box`) plus the `Box::pin` → `pin!` exception on the measured path, and
a pointer to the Callgrind chapter. Cross-links to `naming.md`.

**Open this when**: adding or modifying any file in `crates/<crate>/benches/`;
deciding how to pin a future inside an `iter` closure.

# [Callgrind benchmarks](docs/callgrind-benchmarks.md)

Deep reference for Callgrind / Gungraun instruction-count benchmarks: which
operations to cover, scenario selection, the bench file template, Cargo.toml
setup, Gungraun syntax gotchas, the Criterion-pairing convention, and how to
interpret results.

**Open this when**: adding a `*_cg.rs` benchmark file or deciding whether a hot
path warrants Callgrind coverage.

# [Naming](docs/naming.md)

Naming conventions for benchmark files, Criterion groups, and Callgrind
identifiers — the rules that keep wall-clock and instruction-count benchmarks
in lockstep and prevent name collisions in `target/.../deps/`.

**Open this when**: naming a new benchmark file, group, or function; pairing a
Callgrind file with its Criterion counterpart.

# [Performance](docs/performance.md)

Workspace-wide performance principles: when to add `#[inline]`, the bias
toward surgical interventions over architectural rewrites, preserving
defensive runtime checks, staying idiomatic Rust, deprioritizing
first-insert/teardown optimizations, the no-allocation-on-the-hot-path
reminder, and the rule on justifying deviations from standard ecosystem
patterns.

**Open this when**: considering an `#[inline]` annotation; proposing or
reviewing a performance optimization PR or issue; tempted to reach for a
hand-rolled construct instead of an ecosystem default.

# [Runtime analysis](docs/runtime-analysis.md)

How Miri, `cargo careful`, Loom, and Bolero divide validation responsibility,
including when package/test exclusions or reduced Miri stress cardinalities are
appropriate.

**Open this when**: changing runtime-analysis coverage; adding
`package.metadata.anvil.miri`; ignoring a test under Miri; reducing a test's
workload only under Miri.
