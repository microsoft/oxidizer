# Release planning

This reference contains the model-owned parts of an Oxidizer release. Mechanical
token parsing, version arithmetic, dependency closure, pin reconciliation, and
topological ordering belong to
`.github/skills/release-packages/scripts/resolve-plan.ps1`.

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

Facts use `schemaVersion: 3`; regenerate them when the release tooling changes.
The resolver rejects stale or incomplete facts rather than silently skipping
macro-contract checks. Each package fact contains:

`folder, name, version, published, procMacroOnly, hasLibraryTarget, deps,
exposedDeps, macroPublicDeps, macroImplementationClosure,
macroRuntimePartners, exposureUnknown, baselineSha, hasBaseline, everReleased,
modified, modifiedFiles, modifiedFileCount, workspaceModified`.

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
- `modifiedFiles` is the sorted, repo-relative baseline diff used to audit and
  mechanically constrain selection reasons.
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
- **all**: review every published package. An unchanged package may be released
  only with an explicit token and an `explicit-release` decision.

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
not apply it.

### Deterministic selection decisions

In `changed` mode, record exactly one `selectionDecisions` entry for every
published package where `modified = true`. In `all` mode, record one for every
published package. The resolver rejects missing decisions, decline decisions
that have tokens, accept decisions without tokens, aliases, and extra keys. Use
the canonical `folder` identifier as each key. Invoke the resolver even when all
candidates are declined so it can validate and emit the canonical empty plan.

Judge only the package's own diff when selecting it. Dependency pickup,
exposure, macro-public, and runtime-partner effects belong to the resolver.
Decline a package with no release-worthy own diff even when it will later appear
in the plan with `source: cascade`.

Use the first matching rule:

| Change evidence | Decision | Reason |
|---|---|---|
| Compatibility break in public API, documented behavior, or macro compile contract | accept | `breaking` |
| Backward-compatible public API addition | accept | `nonbreaking-api` |
| Consumer-observable behavior or packaging fix | accept | `behavior-fix` |
| Repair to authored, packaged docs or Rust doc comments | accept | `authored-doc-fix` |
| Normal/build dependency declaration or feature activation changed | accept patch | `runtime-manifest-change` |
| First release with release-worthy packaged content since introduction | accept | `first-release` |
| Unchanged package explicitly requested in `all` mode | accept | `explicit-release` |
| Tests or test-support source plus supporting dev-dependency edits only | decline | `test-only` |
| Benchmarks plus supporting dev-dependency edits only | decline | `benchmark-only` |
| Dev-dependency declaration only | decline | `dev-dependency-only` |
| Lints, docs.rs, `cargo_check_external_types`, release metadata, or formatting only | decline | `release-metadata-only` |
| Generated crate README or generated changelog only | decline | `generated-artifact-only` |
| Internal refactor with proven unchanged observable behavior | decline | `internal-only` |
| No diff from the relevant baseline | decline | `unchanged` |

Crate `README.md` files are generated by `just readme`; never use their diff as
release evidence. Review their originating Rust doc comments instead.
Changelogs are release-generated and likewise never seed a later release.
A normal/build dependency feature change is part of the published manifest and
always seeds a patch, even when it only fixes compilation under a workspace
feature configuration. Dev-dependency features do not.

When several non-release categories are mixed, ignore generated artifacts and
release metadata while classifying the remaining diff. Use `test-only` or
`benchmark-only` when their support edits include dev dependencies; otherwise
use `dev-dependency-only`. If nothing remains, use `release-metadata-only` when
metadata changed, or `generated-artifact-only` when only generated files
changed. Never accept a first release merely because `everReleased = false`;
its own diff must contain release-worthy packaged content.
The resolver rejects `first-release` when every changed path is under `tests/`
or `benches/`, is outside the package allowlist, or is only Cargo/release
metadata or a generated README/changelog. A never-released accepted package must
use the `first-release` reason.

For proc macros, "compile behavior changed" means the end-to-end result changed
for a representative consumer fixture. Parser acceptance alone is not the
contract. Compile the same fixture against the baseline and current package:

- baseline passes, current fails -> breaking;
- baseline fails, current passes -> nonbreaking behavior fix;
- both fail for the same invalid input -> compatible patch unless documented
  diagnostic behavior changed incompatibly;
- both pass -> judge expansion API, behavior, runtime paths, and hygiene.

Do not infer that an old invocation failed merely because its generated code
looks difficult to construct. Record the actual baseline/current command and
exit result in macro evidence. A tool failure or fixture whose prior support
status cannot be established is inconclusive and blocks the plan.

Classify implemented API, not aspirations in TODO, design, or roadmap files.
Such documents can direct investigation but cannot prove a compatibility break.
When `cargo-semver-checks` passes and an `impl Trait` return is replaced by a
newly public concrete type that implements the same trait, treat the new named
type and its additive methods as `nonbreaking` only when it preserves the prior
opaque type's trait, auto-trait (`Send`, `Sync`, `Unpin`), and lifetime-capture
guarantees. Verify uncertain bounds with baseline/current consumer fixtures.

## Resolve the plan

Write request JSON:

```json
{
  "mode": "targeted",
  "tokens": ["bytesbuf@breaking"],
  "selectionDecisions": {},
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

- complete, token-consistent candidate selection in changed/all mode;
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
   cascadeReasons`, plus top-level selection decisions and macro attestations.
   Copy these tuple fields from `plan.json`; do not restate or override
   `manualReview`, warnings, or change types in a separate result summary.
   `manualReview` is resolver-owned: true for proc-macro-only packages and false
   otherwise. Omit it from classifications, or supply only the matching value.
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
  "selectionDecisions": [],
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
