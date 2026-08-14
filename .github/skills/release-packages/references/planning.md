# Release planning

This reference contains the model-owned parts of an Oxidizer release. Mechanical
token parsing, version arithmetic, dependency closure, pin reconciliation, and
topological ordering belong to `scripts/resolve-plan.ps1`.

## Inputs

Use one mode:

- **targeted**: explicit package tokens.
- **changed**: review every published package with unreleased modifications.
- **all**: review every published package.

A token is `name`, `name@breaking`, `name@nonbreaking`, `name@patch`, or
`name@<semver>`. Change types are lower bounds. A version is an exact pin and
must be strictly greater than the current version.

## Facts

Run:

```powershell
./.github/skills/release-packages/scripts/release-facts.ps1 > facts.json
```

Facts use `schemaVersion: 2`; regenerate them when the release tooling changes.
The resolver rejects stale or incomplete facts rather than silently skipping
macro-contract checks. Each package fact contains:

`folder, name, version, published, procMacroOnly, hasLibraryTarget, deps,
exposedDeps, macroPublicDeps, macroImplementationClosure,
macroRuntimePartners, exposureUnknown, baselineSha, hasBaseline, everReleased,
modified, modifiedFileCount, workspaceModified`.

- `deps` contains normalized non-dev dependency names.
- `exposedDeps` contains direct or transitively reachable workspace packages
  whose defining types appear in the package's public API. Fact gathering
  resolves dependency aliases and custom `[lib] name` crate roots.
- `macroPublicDeps` contains proc-macro packages whose entry points are
  positively identified by a concrete root in the package's public allowlist.
  Wildcards are not positive publication evidence. These are behavioral
  contracts, not Rust type-identity exposure.
- `macroImplementationClosure` contains workspace dependencies reachable from a
  proc-macro package. It defines the source-diff review scope.
- `workspaceModified` includes unpublished workspace packages. `modified`
  remains publishable-only for release selection, while proc-macro review uses
  the broader fact so private implementation helpers cannot bypass attestation.
- `macroRuntimePartners` is inferred by reversing `macroPublicDeps`: every
  package that publicly exposes a proc macro becomes its runtime façade.
  `[package.metadata.oxidizer_release].macro_runtime` is an optional escape
  hatch for generated relationships without a public façade edge.
  A macro attestation that marks `generatedRuntimePaths` as changed blocks
  resolution when no partner was inferred or declared, preventing a new macro
  crate from silently omitting the relationship.
- `exposureUnknown` remains true for unchecked non-library targets. Proc-macro
  packages set it false because rustc prevents dependency types from crossing
  a proc-macro boundary.
- For ordinary libraries, missing metadata fails closed on direct dependencies;
  an explicit empty allowlist proves no direct exposure. Indirect exposure
  requires positive allowlist evidence.
- Use `everReleased`, not `hasBaseline`, to identify a first release. A crate's
  introducing commit also counts as a version-bump baseline.

Never hand-parse Cargo metadata or reconstruct these facts.

## Objective classification

Build a classification map for every package that may enter the release set.

### Previously released ordinary libraries

Run:

```text
cargo semver-checks --package <name> --baseline-rev <baselineSha> \
  --all-features --color never
```

Map the result:

| Result | Classification |
|---|---|
| major bump required | `breaking` |
| minor bump required | `nonbreaking` |
| compatible / no update required | `patch` |

Tool or build failures are fatal. Never silently classify them as patch.
`cargo semver-checks` proves compatibility but may not identify a new public API
as requiring a minor bump. Source-diff review must elevate such additions to
`nonbreaking`.

### First-ever releases

When `everReleased = false`, do not run `cargo semver-checks` against the
introducing commit. The first release uses the version already declared in
`Cargo.toml`.

### Proc-macro-only packages

Implementation dependency versions are not the proc-macro contract. Review the
proc-macro package, modified members of its `macroImplementationClosure`, and
affected `macroRuntimePartners`. The consumer contract includes:

- exported macro names;
- derive helper attributes;
- accepted syntax and compile success/failure;
- generated behavior, public API, bounds, and implementations;
- generated runtime paths and requirements;
- hygiene and name resolution.

Diagnostic wording and span changes are patch unless documented as contractual.
Changing accepted input into a compile failure is breaking. Token formatting is
not a contract; judge behavior-equivalent, not byte-equivalent, expansion.

Record `macroContracts.<package>` with:

- `verdict`: `compatible`, `nonbreaking`, or `breaking`;
- `reviewedPackages`: every package in the resolver's review scope;
- all required contract channels classified as `unchanged`, `changed`, or
  `notApplicable`;
- concrete evidence such as normalized expansion snapshots, trybuild pass/fail
  fixtures, generated-runtime compile tests, and exported entry-point review.

The verdict is the objective classification for the proc-macro contract.
`manualReview` always remains true. A published implementation library still
receives its own independent Rust API classification; `#[doc(hidden)]` does not
remove its SemVer obligations.

## Selecting packages

Snapshot published, modified packages before resolving any cascade.

- **targeted**: explicit tokens are accepted. Review every other package in the
  modified snapshot.
- **changed**: review the modified snapshot and accept packages with
  consumer-visible changes.
- **all**: review every published package, but do not release an unchanged
  package without an explicit token.

For a reviewed package:

1. Inspect `git diff <baselineSha>..HEAD -- crates/<folder>` plus working-tree
   changes.
2. Default to the objective classification.
3. Elevate only with concrete evidence the tool cannot see:
   - documented behavioral incompatibility -> breaking;
   - missed public signature or type incompatibility -> breaking;
   - a major dependency upgrade used in exposed public types -> breaking;
   - narrowed generic or auto-trait implementation bounds -> breaking;
   - missed backward-compatible public addition -> nonbreaking.
4. Treat packaged documentation repairs that fix broken links or incorrect
   consumer guidance as patch changes.
5. Decline packages with no consumer-visible change. Opaque generated README
   metadata and dependency-version link refreshes do not seed a release when
   they are only byproducts of another package's planned release.

Never elevate by taste. Cite the file and public item or behavior.
If every changed or all candidate is declined, stop with an empty plan and do
not invoke the resolver.

## Resolve the plan

Write request JSON:

```json
{
  "mode": "targeted",
  "tokens": ["bytesbuf@breaking"],
  "classifications": {
    "bytesbuf": "patch",
    "bytesbuf_io": {
      "changeType": "patch",
      "manualReview": false
    }
  },
  "macroContracts": {
    "templated_uri_macros": {
      "verdict": "compatible",
      "reviewedPackages": [
        "templated_uri_macros",
        "templated_uri_macros_impl"
      ],
      "channels": {
        "exportedMacros": "unchanged",
        "acceptedSyntax": "unchanged",
        "compileBehavior": "unchanged",
        "generatedApi": "unchanged",
        "generatedRuntimePaths": "unchanged",
        "hygiene": "unchanged"
      },
      "evidence": [
        "Normalized expansion snapshots and compile fixtures are unchanged."
      ]
    }
  },
  "force": false
}
```

Include classifications for every previously released ordinary library reachable
from accepted tokens. The resolver fails rather than guessing a missing
classification; classify the complete dependent closure named by the failure and
rerun against the same facts.

Run:

```powershell
./.github/skills/release-packages/scripts/resolve-plan.ps1 `
  -FactsPath facts.json -RequestPath request.json > plan.json
```

The resolver guarantees:

- strict SemVer token and pin validation;
- the version rules in `version-rules.md`;
- transitive published-dependent closure;
- exclusion of never-published dependents from cascades;
- patch floor for every dependency pickup;
- breaking propagation through ordinary type exposure and reviewed public macro
  contracts;
- structured blocking when a required macro contract is absent or incomplete;
- fixed-point propagation through chains and diamonds;
- proc-macro manual-review preservation;
- pin-versus-requirement rejection unless `force` is true;
- dependency-before-dependent ordering;
- deterministic, sorted cascade reasons.

The resolver has three independent cascade lanes:

1. Every direct Cargo dependency release gives a patch floor.
2. An ordinary type edge is breaking when the dependency's version transition
   is Cargo-incompatible and the dependent lists it in `exposedDeps`, or is a
   direct dependent with `exposureUnknown = true`.
3. A macro implementation edge never breaks automatically. Its reviewed
   `macroContracts` verdict classifies the proc macro. A proc-macro dependency
   breaks a dependent only when the contract verdict is breaking and the
   dependent lists it in `macroPublicDeps`.

A compatible proc-macro contract stays at the patch floor even when its
implementation dependency or exact pinned version crosses a Cargo major line.
An unresolved required contract returns `status: blocked` with sorted
`ambiguities`; no partial release plan may be applied.

When a breaking runtime release matches a proc macro's
`macroRuntimePartners`, the resolver requires a macro-contract review. A
compatible verdict records the review without forcing a macro release; a
nonbreaking or breaking verdict adds the macro to the release set.

`force` never permits a downgrade or a pin equal to the current version. When it
keeps a pin below a computed requirement, the resolver records a warning and
retains the stronger effective change type for further cascades.

## Consensus gate

Before writing repository files:

1. Freeze facts, tokens, classifications, reviewed diffs, and `plan.json`.
2. Ask at least two additional model families to independently review the
   classifications and verify the plan follows from them.
3. Compare normalized tuples:
   `folder, to, changeType, source, manualReview, contractBreaking,
   cascadeReasons`, plus the top-level macro attestations.
4. Continue only on unanimous agreement.

On divergence, stop and report:

- package and rule that diverged;
- each model's decision and evidence;
- the ambiguous sentence;
- a concrete proposed edit to this skill;
- recommended human action.

Do not average, vote, or choose the most conservative plan silently.
The resolver is the canonical authority for arithmetic, cascades, pins, and
ordering. Models verify its inputs and output; they do not replace it with
hand-computed plans.

## Apply atomically

Run:

```powershell
./.github/skills/release-packages/scripts/apply-plan.ps1 -PlanPath plan.json
```

The helper edits only package and workspace dependency version values, generates
changelogs and READMEs, validates with Cargo, verifies applied versions, and
restores every file it touched if any operation fails.

## Canonical output

```json
{
  "status": "resolved",
  "mode": "targeted",
  "releases": [
    {
      "folder": "bytesbuf",
      "name": "bytesbuf",
      "from": "0.8.0",
      "to": "0.9.0",
      "changeType": "breaking",
      "source": "user",
      "manualReview": false,
      "contractBreaking": false,
      "cascadeReasons": []
    }
  ],
  "macroContracts": [],
  "ambiguities": [],
  "warnings": [],
  "consensus": {
    "models": ["model-a", "model-b", "model-c"],
    "agreement": "unanimous"
  }
}
```
