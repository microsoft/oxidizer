# Releasing Oxidizer Packages

Oxidizer package releases are planned and applied by the repository skill at
`.github/skills/release-packages/SKILL.md`. The skill owns orchestration and
source-diff judgment; small PowerShell helpers own deterministic mechanics.

## Architecture

| Component | Responsibility |
|---|---|
| `release-facts.ps1` | Workspace packages, versions, dependencies, type exposure, macro relationships, release baselines, and modifications |
| `resolve-plan.ps1` | Tokens, SemVer arithmetic, pins, type/macro contract cascades, ambiguities, and topological ordering |
| `apply-plan.ps1` | Version writes, changelogs, README generation, Cargo validation, and rollback |
| `release-changelog.ps1` | One deterministic changelog |
| `scripts/ci/semver-report.ps1` | CI report for version changes already present in a PR |

Shared functions remain in `scripts/lib/releasing.ps1` and
`scripts/lib/changelog.ps1` because CI also consumes them.

The model may classify source changes and procedural macro contracts. It must not
reimplement version arithmetic, dependency closure, Cargo.toml editing, or
rollback.

## Terminology

- **Dependency**: a package consumed by another package.
- **Dependent**: a package that consumes another package.
- **Direct**: one dependency edge away.
- **Transitive**: reachable through one or more dependency edges.
- **Change type**: `breaking`, `nonbreaking`, or `patch`.
- **Release set**: explicit releases plus published dependents pulled in by the
  cascade.
- **User-source release**: selected explicitly during review.
- **Cascade-source release**: added because one of its dependencies is released.
- **First release**: a publishable package with no matching release tag;
  `everReleased`, not `hasBaseline`, identifies this state.

Avoid *upstream* and *downstream* because their direction is ambiguous.

## Modes and tokens

The skill supports:

- **targeted**: explicit package tokens;
- **changed**: review every publishable package with unreleased changes;
- **all**: review every publishable package.

A token is:

```text
name
name@breaking
name@nonbreaking
name@patch
name@<semver>
```

Change types are lower bounds. An explicit version is an exact pin and must be
strictly greater than the package's current version under SemVer precedence.
Build metadata does not affect precedence.

If every changed or all candidate is declined, the result is an empty plan and
nothing is written.

## Version rules

| Current | breaking | nonbreaking | patch |
|---|---|---|---|
| `x.y.z`, `x >= 1` | `(x+1).0.0` | `x.(y+1).0` | `x.y.(z+1)` |
| `0.y.z`, `y >= 1` | `0.(y+1).0` | `0.y.(z+1)` | `0.y.(z+1)` |
| `0.0.z` | `0.0.(z+1)` | `0.0.(z+1)` | `0.0.(z+1)` |

On `0.y.z`, nonbreaking and patch retain distinct intent despite producing the
same version. Every `0.0.z` transition is breaking under Cargo compatibility.
See the skill's `references/version-rules.md` for the executable resolver's
canonical rules.

## Release facts and exposure

`release-facts.ps1` emits, for each package:

```text
folder, name, version, published, procMacroOnly, hasLibraryTarget,
deps, exposedDeps, macroPublicDeps, macroImplementationClosure,
macroRuntimePartners, macroCompileFixtureChanges, externalDepChanges,
externalExposedDeps, exposureUnknown,
baselineSha, hasBaseline, everReleased, modified, modifiedFiles,
modifiedFileCount, manifestDependencyScopes, manifestOtherChanged,
rustImplementationChanged, workspaceModified
```

`deps` contains normalized normal and build dependencies; dev dependencies are
excluded.

`modified` remains publishable-only for changed-mode selection.
`workspaceModified` also records unpublished workspace changes so proc-macro
review cannot skip a private implementation helper that changed.
`modifiedFiles` records the sorted baseline-diff paths and lets the resolver
reject a first release justified only by tests, benchmarks, or generated files.
The paths come from one frozen published/unpublished workspace scan and are
ordered ordinally.
`manifestDependencyScopes` records whether changed dependency declarations are
normal, build, or dev scoped, and whether package features changed. Selection
validation uses it to prevent dev-only manifest edits from becoming release
seeds.
`manifestOtherChanged` distinguishes a pure dev-dependency edit from another
mixed manifest change without deciding whether that other edit requires a
release; lints and `[package.metadata]` remain ignorable release metadata.
`rustImplementationChanged` is true only when the crate's own packaged Rust
source changed beyond doc comments (a non-comment line in a `.rs` file under
`src/`, a custom `[lib]` path, or `build.rs`, never `tests/`/`benches/`/
`examples/`). A previously released library with it false and no exposed
breaking external dependency change cannot be classified `breaking` or
`nonbreaking` on its own account -- a re-exported macro contract break or a
dependency bump reaches it only as a resolver-owned cascade. It fails safe: a
missing baseline or an untracked new source file counts as changed.

For ordinary libraries, public exposure is derived from
`package.metadata.cargo_check_external_types.allowed_external_types`. Fact
gathering resolves dependency aliases and custom `[lib] name` crate roots before
matching allowlist entries. It also records a transitively reachable workspace
package in `exposedDeps` when an allowlist positively identifies that defining
crate through a re-export, even when no direct dependency edge exists.

Exposure handling is conservative:

- absent metadata fails closed for direct dependencies because working-tree
  changes may not have passed CI yet;
- an explicit empty allowlist proves that no direct dependency type is exposed;
- malformed or wildcard direct entries fail closed;
- indirect exposure requires positive allowlist evidence, so absent or malformed
  metadata does not mark every transitive dependency as exposed.

Proc-macro-only packages cannot expose dependency Rust types: rustc restricts
their public surface to proc-macro entry points. They therefore have no
`exposedDeps` and do not set `exposureUnknown`. Public macro re-exports are recorded separately in `macroPublicDeps` only when
a concrete allowlist root identifies the direct proc-macro dependency.
Wildcards are not positive publication evidence.

`macroImplementationClosure` identifies workspace code that can change a macro's
behavior. `macroRuntimePartners` is inferred by reversing `macroPublicDeps`: a
package that publicly exposes a proc macro is treated as its runtime façade.
`[package.metadata.oxidizer_release].macro_runtime` remains an escape hatch for
generated-runtime relationships without a public façade edge.
If a macro attestation marks generated runtime paths as changed but no partner
was inferred or declared, resolution blocks with `macroRuntimeUnknown`.

`macroCompileFixtureChanges` inventories the compile fixtures that changed in a
proc macro's review scope: `tests/ui/**` and `tests/compile_fail/**` `.rs` cases
and their `.stderr`/`.stdout` expectations, owned by the macro, its modified
implementation closure, or its modified runtime partners. The cross-package
reach is the point. A fixture proving that a macro now rejects input it used to
accept normally lives in the runtime façade, where it reads as a plain test-only
edit. Each entry records `ownerPackage`, `ownerPublished`, `path`, `kind`,
`status` (`added`/`modified`/`removed`), `expectedResult`, `baselineRev`, and
`scopeRole`, ordered deterministically.

## External dependency exposure

A crate's registry dependency requirements ship inside its published manifest,
so consumers resolve against them directly. `cargo semver-checks` compares this
workspace's own rustdoc and cannot see that a public signature now names a type
from a different major version of a third-party crate. `externalDepChanges`
closes that gap mechanically.

Current requirements come from `cargo metadata`, which resolves
`workspace = true` inheritance, renames, and target-specific tables. Baseline
requirements come from the package manifest and root `[workspace.dependencies]`
at its own `baselineSha` -- read from Git, because `cargo metadata` cannot
inspect a revision without materializing it. Both sides are normalized the way
cargo normalizes a bare version before comparison. Dev dependencies and
workspace members are excluded; workspace members are already covered by
`deps` and the cascade.

Each entry records `name`, `baselineReq`, `currentReq`, `kinds`, `breaking`,
and `baselineRev`, sorted ordinally by name. `breaking` is decided by the Cargo
compatibility line -- the leading non-zero component span that governs
unification:

| Transition | `breaking` |
|---|---|
| `^2.0.111` to `^2.9.0` | false |
| `^0.5.1` to `^0.5.9` | false |
| dependency added | false |
| `^2.0.111` to `^3.0.2` | true |
| `^0.5.1` to `^0.6.0` | true |
| dependency removed | true |
| either side unreadable (`*`, comparator ranges, conflicting terms) | true |

Because a requirement can only change through an edited manifest, a package
whose sole change is an inherited `[workspace.dependencies]` bump is promoted to
`modified` and `workspaceModified`, and the affected scope is added to
`manifestDependencyScopes`. `cargo publish` inlines the inherited value, so its
published manifest genuinely changed even though nothing under
`crates/<folder>/` was touched.

`externalExposedDeps` applies the `cargo_check_external_types` allowlist to the
package's current external dependencies, with the same fail-closed rules used
for `exposedDeps`: absent or malformed metadata exposes every external
dependency, an explicit empty allowlist exposes none. Proc-macro-only packages
always report an empty list -- a proc macro exports behavior, and rustc keeps
foreign type identity from crossing the macro boundary. Their dependency
upgrades are judged by the macro contract instead.

The resolver imposes a floor when a breaking change names a dependency in
`externalExposedDeps`: the classification must be `breaking`
(`externalExposureUnderclassified` otherwise) and the selection reason must be
`breaking`, including for a decline (`externalExposureUnderselected` otherwise).
First-ever releases are exempt, having no prior requirement to invalidate.

Facts use `schemaVersion: 5`. The resolver rejects older or incomplete facts
instead of silently disabling macro-contract checks; regenerate facts after
updating the release tooling.

## Classification

For every previously released ordinary library that may enter the release set:

```text
cargo semver-checks --package <name> --baseline-rev <baselineSha> \
  --all-features --color never
```

The baseline is the most recent reachable commit that changed the package's
declared version. It is rebuilt from repository history, so no registry access is
required.

Map detected compatibility requirements to `breaking`, `nonbreaking`, or
`patch`. Tool and build failures are fatal. `cargo semver-checks` proves
compatibility but does not catch every public signature/type change and may not
identify a new public API as requiring a minor bump. Source-diff review must
elevate missed incompatibilities to `breaking` and backward-compatible additions
to `nonbreaking`. In particular, review manual auto-trait implementations and
their generic bounds: replacing structural derivation can remove implementations
for previously accepted type arguments without being reported by the tool.
Likewise, a major dependency upgrade is breaking when that dependency appears in
the crate's exposed public types, even if the crate-local Rust source is
unchanged.

Packaged documentation repairs that fix broken links or incorrect consumer
guidance are patch changes. Opaque generated README metadata and dependency-link
version refreshes do not independently seed a release when they are only
byproducts of another package's planned release.

First releases do not run against their introducing commit. They publish at the
version already declared in `Cargo.toml`, unless explicitly pinned higher.

Proc-macro-only packages require a `macroContracts` attestation covering:

- exported macro names and derive helper attributes;
- accepted syntax and compile success/failure;
- generated behavior, public API, bounds, and implementations;
- generated runtime paths and requirements;
- hygiene and name resolution.

The verdict is `compatible`, `nonbreaking`, or `breaking`. It includes reviewed
packages, channel decisions, and concrete evidence. Diagnostic wording and token
formatting are patch unless they alter documented behavior. `manualReview`
remains true.

For changed/all selection, the request records an evidenced accept/decline
decision for every candidate under its canonical folder key. Generated crate READMEs and changelogs, tests,
benchmarks, dev dependencies, and release-only metadata do not seed releases.
Normal/build dependency declaration or feature changes do seed a patch because
they change the published manifest. Authored Rust docs may seed a patch.
Selection considers only the package's own diff; cascades are resolver-owned.
An all-declined decision set is still resolved and emitted as an empty plan.

A `behavior-fix` accept must be measured, not narrated. Its decision carries
`regressionEvidence`: one or more `consumer-runtime`, `consumer-compile`, or
`packaged-artifact` probes, each recorded at the release baseline and at the
current revision with a `pass`/`fail` `result`, the `revision` measured, and the
process `exitCode`. Only a baseline failure that now passes demonstrates the
fix. No probe, or a probe whose outcome did not improve, blocks with
`behaviorFixUndemonstrated`; a measurement that is incomplete, contradicts its
own exit code, or compares one revision against itself blocks with
`behaviorEvidenceInconclusive`. Both produce an empty release plan, so an
internal adaptation that preserves observable behavior cannot seed a release —
it is `internal-only`. Other selection reasons are unaffected.

Proc-macro compile compatibility is measured with the same consumer fixture
against baseline and current packages. A baseline pass that becomes a current
failure is breaking; parser acceptance without end-to-end evidence is not a
separate contract.

Those measurements are not free text. Every fixture in
`macroCompileFixtureChanges` must appear in the contract's `compileEvidence`
with a structured `baseline` and `current` (`result` of `pass`/`fail`, the
`revision` measured, and the compiler `exitCode`); a `.stderr`/`.stdout`
obligation is discharged by measuring its `.rs` sibling. The resolver derives a
verdict floor from those outcomes — pass→fail is breaking, fail→pass is
nonbreaking, an unchanged outcome is compatible — and blocks a declared verdict
below the floor with `macroVerdictUnderclassified`. Unmeasured fixtures block
with `macroCompileFixtureUnevidenced` and unusable measurements with
`macroCompileEvidenceInconclusive`, each producing an empty release plan. A
derived break additionally forbids declining the package or justifying it with a
weaker selection reason. Fixtures owned by a published implementation dependency
must still be measured but do not set the macro's floor, because that crate
carries its own independent classification.

A breaking external dependency requirement change on a dependency in
`externalExposedDeps` imposes the same kind of floor without any recorded
evidence to weigh: the classification and the selection reason must both be
`breaking`. Private external dependencies and proc-macro-only packages are
untouched by this rule.

Compatibility classification follows implemented API and verified consumer
behavior, not TODO/design claims. A passing SemVer check plus an
`impl Trait`-to-concrete return refinement remains nonbreaking when the concrete
type implements the same trait and consumer probes show no regression.
The concrete type must also preserve prior auto-trait and lifetime-capture
guarantees.

## Cascade rules

Every released dependency gives each previously released, publishable direct
dependent a patch floor so it can pick up the new dependency requirement.
Never-published dependents are not cascade releases.

A package receives an ordinary Rust type breaking floor when:

1. the dependency's actual version transition is breaking under Cargo
   compatibility; and
2. the package exposes that dependency through `exposedDeps`, or is a direct
   dependent with `exposureUnknown = true`.

The exposure edge may be direct or may identify a transitively reachable
defining crate through a public re-export. Indirect packages receive no patch
floor for compatible dependency changes because they do not declare the
dependency requirement themselves.

This is a fixed-point calculation. A strengthened package can strengthen its own
dependents, including through chains and diamonds. Every `0.0.z` bump therefore
propagates as breaking across ordinary type-exposure edges even when its source
classification was patch.

The release set is ordered dependency before dependent. Duplicate normal/build
edges are deduplicated. Unpublished packages are excluded.

Proc-macro cascades are separate:

- an implementation dependency release gives the proc macro a patch floor;
- its Cargo-incompatible version does not imply a broken macro contract;
- a required but missing macro review blocks resolution instead of guessing;
- a reviewed breaking macro contract propagates only through
  `macroPublicDeps`;
- a private proc-macro dependency remains a patch pickup;
- generated-runtime relationships require review when the runtime breaks.

The plan records `contractBreaking`, edge class, judgment, and judgment source
so macro decisions remain auditable.

## Pins and force

A proc-macro exact pin still requires a contract attestation because version
arithmetic cannot prove behavioral compatibility. A pin below its objective or
cascade requirement is rejected. With `force: true`,
the resolver retains the exact pin, emits a warning, and preserves the stronger
effective change type for downstream cascade decisions.

Force never permits a downgrade or a pin equal to the current version.

## Consensus

Before applying a plan, freeze facts, classifications, evidence, request JSON,
and resolver output. At least two additional model families review the
classifications and verify that the resolver output follows from them.

The resolver is authoritative for arithmetic, cascades, pins, and ordering.
Models do not independently replace it with hand-computed plans. Any
classification or rule disagreement stops the release instead of being averaged
or silently resolved.

## Atomic application

Apply a resolved plan with:

```powershell
./.github/skills/release-packages/scripts/apply-plan.ps1 -PlanPath plan.json
```

The helper:

1. verifies each package and workspace dependency is still at the plan's `from`
   version;
2. edits only `[package].version` and the existing workspace dependency inline
   table's `version` value;
3. generates changelogs in dependency-first order;
4. runs `just readme` once;
5. runs Cargo metadata and workspace checks;
6. verifies every applied package and workspace dependency version.

It snapshots package manifests, the root manifest and lockfile, changelogs, and
all possible crate README paths. Any failure restores modified files and removes
files created by the failed release.

Do not publish packages unless publication was explicitly requested.

## CI SemVer report

`.github/workflows/main.yml` invokes `scripts/ci/semver-report.ps1` for package
version changes in a PR. It uses the same release-baseline and proc-macro
semantics but remains outside the skill because it is a CI entry point.

The report verifies whether versions already written in the PR are sufficient.
It does not replace release planning or source-diff review.

## Tests

Mechanical behavior is covered by the Pester suite:

- stable, `0.x`, and `0.0.x` arithmetic;
- patch, additive, and breaking synthetic Cargo changes;
- linear, diamond, duplicate-edge, renamed-root, direct, indirect, and
  unknown-exposure graphs;
- first releases, unpublished packages, proc-macros, pins, and force;
- changelog rendering;
- successful application and rollback after validation failure.

Run:

```powershell
pwsh -NoProfile -File scripts/tests/Pester/Run-Tests.ps1
```
