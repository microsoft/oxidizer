# AI Agents Guidelines

Code in this repository should follow the guidelines specified in the [Microsoft Rust Guidelines](https://microsoft.github.io/rust-guidelines/agents/all.txt).

## README Files

Crate README files are auto-generated via `just readme`. Do not manually update them. Because they are generated, do not edit them by hand and do not raise their formatting, wording, or casing (for example the title-cased crate name) as issues in code review - such content is not authored here and is regenerated from the crate's doc comments.

## Executing `just` commands

If you only touch one package, you may use `just package=PACKAGE_NAME command` to narrow command scope to that package, where PACKAGE_NAME is the Cargo.toml `[package].name`, which may differ from the directory name.

## Pre-commit Checklist

- Run `just clippy` to verify the code compiles without linter errors.
- Run `just format` to format code.
- Run `just readme` to regenerate crate-level readme files.
- Run `just spellcheck` to check spelling in code comments and docs.

## Spelling

The spell checker dictionary is in the `.spelling` file, one word per line in arbitrary order.

## Changelogs

The changelogs are updated by `scripts/release-packages.ps1` at release time, based on Git history. It is not necessary to make manual edits
to the changelogs, though you are permitted to do so if explicitly instructed.

## Releasing Packages

See [docs/releasing.md](docs/releasing.md) for the release tooling
reference: glossary (direct/transitive dependent vs dependency, cascade
direction, change type vs version component, release set, pending
release, elevation), the cascade-organisation invariants, and the
workflow for `scripts/release-packages.ps1`.

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

Shared fixtures that a crate's own tests, benchmarks and examples need - mock servers, scripted backends - must not become public API, and must not be hosted in a package that depends on the crate under test, because that is a dependency cycle. Host them in the crate's implementation package behind a `private-test-util` feature that the public facade enables through a dev-dependency. See [docs/private-test-utils.md](docs/private-test-utils.md).

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

## Required CI Checks

The `required-checks` job in `.github/workflows/main.yml` is a "fan-in"
aggregator: branch protection requires only this single context for jobs
defined in that workflow, and it succeeds when every dependency either
succeeded or was skipped.

When you add a new job to `main.yml`, you MUST also add it to the `needs:`
list of `required-checks` if it has BOTH a `strategy.matrix` AND a
job-level `if:` that can evaluate to false (typically gated on
`needs.delta.outputs.skip` or `github.event_name`). GitHub Actions does
not expand the matrix when such a gate skips the job, so per-OS contexts
like `testing (ubuntu-latest)` are never posted and would stay stuck on
`Expected — Waiting for status to be reported` if required directly.

Other required jobs should also be funnelled through `required-checks`
so branch protection only references one workflow context. See the
inline comment on the `required-checks` job for the full policy.

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

How to test `tracing` output without cross-test pollution. Key rule: every test
binary that emits or inspects `tracing` must invoke `testing_aids::init_tracing!()`
at module scope, or trace-event lines may be reported as uncovered even though
they run.

**Open this when**: writing or moving any test that inspects `tracing` output;
adding a log/event emission that needs coverage; adding a crate whose tests emit
`tracing` events (it needs the ctor initialization); tempted to install a global
subscriber in a unit test.

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
