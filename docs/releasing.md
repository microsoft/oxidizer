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
macroRuntimePartners, exposureUnknown, baselineSha, hasBaseline,
everReleased, modified, modifiedFileCount, workspaceModified
```

`deps` contains normalized normal and build dependencies; dev dependencies are
excluded.

`modified` remains publishable-only for changed-mode selection.
`workspaceModified` also records unpublished workspace changes so proc-macro
review cannot skip a private implementation helper that changed.

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

Facts use `schemaVersion: 2`. The resolver rejects older or incomplete facts
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
