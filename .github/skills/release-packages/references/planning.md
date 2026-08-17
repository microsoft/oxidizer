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

Facts use `schemaVersion: 5`; regenerate them when the release tooling changes.
The resolver rejects stale or incomplete facts rather than silently skipping
macro-contract checks. Each package fact contains:

`folder, name, version, published, procMacroOnly, hasLibraryTarget, deps,
exposedDeps, macroPublicDeps, macroImplementationClosure,
macroRuntimePartners, macroCompileFixtureChanges, externalDepChanges,
externalExposedDeps, exposureUnknown, baselineSha,
hasBaseline, everReleased, modified, modifiedFiles, modifiedFileCount,
manifestDependencyScopes, manifestOtherChanged, rustImplementationChanged,
workspaceModified`.

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
- `manifestDependencyScopes` mechanically identifies changed `normal`, `build`,
  and `dev` dependency declarations plus package `features`. The resolver rejects a
  `runtime-manifest-change` without a normal/build dependency or package-feature
  change and rejects
  `dev-dependency-only` when authored files or runtime dependencies also changed.
- `manifestOtherChanged` records a semantic change elsewhere in `Cargo.toml`,
  excluding lints and `[package.metadata]`. This distinguishes mixed manifest
  edits from a pure dev-dependency change without deciding whether the other
  edit itself requires a release.
- `rustImplementationChanged` is true only when the crate's own packaged Rust
  source changed beyond doc comments -- an added or removed non-comment line in a
  `.rs` file under `src/`, a custom `[lib]` path, or `build.rs`, never under
  `tests/`, `benches/`, or `examples/`. Doc-comment, test, benchmark, README,
  changelog, and manifest edits leave it false. A previously released ordinary
  library with this false and no exposed breaking external dependency change has
  no own-diff basis for a `breaking` or `nonbreaking` classification; any
  elevation above patch must come from a resolver-owned cascade, not from a
  re-exported macro contract or a dependency bump read into the crate's own diff.
  It fails safe: a missing baseline or a brand-new untracked source file counts
  as an implementation change.
- `macroRuntimePartners` is inferred by reversing `macroPublicDeps`: every
  package that publicly exposes a proc macro becomes its runtime façade.
  `[package.metadata.oxidizer_release].macro_runtime` is an optional escape
  hatch for generated relationships without a public façade edge.
  A macro attestation that marks `generatedRuntimePaths` as changed blocks
  resolution when no partner was inferred or declared, preventing a new macro
  crate from silently omitting the relationship.
- `macroCompileFixtureChanges` lists, for each proc-macro package, every
  compile-fixture path that changed in its review scope: `tests/ui/**` and
  `tests/compile_fail/**` `.rs` cases plus their `.stderr`/`.stdout`
  expectations, gathered from the macro itself, its modified
  `macroImplementationClosure`, and its modified `macroRuntimePartners`. This
  crosses package boundaries on purpose: a fixture that proves a macro now
  rejects previously accepted input usually lives in the runtime façade, where
  it is otherwise indistinguishable from an ordinary test-only edit. Each item
  carries `ownerPackage`, `ownerPublished`, `path`, `kind`
  (`uiFixture`/`uiExpectation`), `status` (`added`/`modified`/`removed`),
  `expectedResult` (`fail` when a recorded expectation exists on either side,
  otherwise null), `baselineRev` (the revision the diff was taken against), and
  `scopeRole` (`self`, `runtimePartner`, or `implementationClosure`). Items are
  sorted ordinally by owner then path.
- `externalDepChanges` lists every effective non-dev **external** (registry)
  dependency requirement that differs between the package's release baseline
  and the working tree, with `name`, `baselineReq`, `currentReq`, `kinds`
  (`normal`/`build`), `breaking`, and `baselineRev`, sorted ordinally by name.
  Requirements are compared after cargo's own normalization, and
  `[workspace.dependencies]` inheritance is resolved on both sides, so a root
  `Cargo.toml` bump is attributed to every crate that inherits it.
  `breaking` is true when the requirement leaves the Cargo compatibility line
  it was released against (`^2.0.111` to `^3.0.2`, `^0.5.1` to `^0.6.0`), when
  the dependency was dropped, or when either requirement cannot be read well
  enough to decide. A move within one line (`^2.0.111` to `^2.9.0`) and a newly
  added dependency are not breaking.
  A package whose only change is an inherited requirement is promoted to
  `modified`/`workspaceModified` with the affected scope added to
  `manifestDependencyScopes`: `cargo publish` inlines the inherited value, so
  its published manifest really did change even though no file under
  `crates/<folder>/` was touched.
- `externalExposedDeps` lists the current external dependencies whose types the
  package's public API may name, derived from
  `[package.metadata.cargo_check_external_types].allowed_external_types` with
  the same fail-closed rules as `exposedDeps`. It is always empty for
  proc-macro-only packages: a macro exports behavior, and rustc keeps foreign
  type identity from crossing the macro boundary.
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
  fixtures, generated-runtime compile tests, and exported entry-point review;
- `compileEvidence`: one measured entry per fixture in
  `macroCompileFixtureChanges`.

### Compile evidence

Every fixture the facts report is an obligation the contract must discharge.
Each `compileEvidence` entry is:

```json
{
  "ownerPackage": "ohno",
  "path": "crates/ohno/tests/ui/ohno_error_no_constructors.rs",
  "baseline": { "result": "pass", "revision": "<baselineRev>", "exitCode": 0 },
  "current":  { "result": "fail", "revision": "<HEAD>", "exitCode": 101 }
}
```

Measure each fixture by compiling it at both revisions; `result` is `pass` or
`fail` and must be accompanied by the revision measured and the compiler exit
code. A `.stderr`/`.stdout` obligation is discharged by measuring its `.rs`
sibling, so one measurement covers the whole fixture group.

The resolver reads those two outcomes mechanically:

| Baseline | Current | Derived verdict floor |
|---|---|---|
| pass | fail | `breaking` |
| fail | pass | `nonbreaking` |
| pass | pass | `compatible` |
| fail | fail | `compatible` |

The declared `verdict` may sit at or above the strongest derived floor, never
below it. Fixtures owned by a *published* member of `macroImplementationClosure`
are still reported and must still be measured, but do not set the floor: that
crate carries its own independent classification.

The verdict is the objective classification for the proc-macro contract.
`manualReview` always remains true. A published implementation library still
receives its own independent Rust API classification; `#[doc(hidden)]` does not
remove its SemVer obligations.

### External dependency exposure

A crate's external dependency requirements are part of its published manifest,
so a consumer resolves against them directly. Moving one to another
compatibility line while the crate's public API names that dependency's types
hands every consumer a different type identity under unchanged paths --
invisibly to `cargo semver-checks`, which only sees this workspace's rustdoc.

The resolver derives the floor mechanically, with no judgement to record:

| `externalDepChanges` entry | In `externalExposedDeps` | Derived floor |
|---|---|---|
| `breaking: true` | yes | `breaking` |
| `breaking: true` | no (private dependency) | none |
| `breaking: false` | either | none |
| any (proc-macro-only package) | never (always empty) | none |

A classification below that floor blocks with `externalExposureUnderclassified`;
a selection reason other than `breaking` -- including any decline -- blocks with
`externalExposureUnderselected`. Both list the dependencies and both
requirements, and neither can be argued away: raise the classification and the
reason, or revert the requirement. Crates with `everReleased: false` are exempt,
because a first release has no prior requirement to invalidate.

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
Accordingly, a `breaking` selection reason must agree with that package's own
objective classification. A runtime facade must not label itself breaking only
because a re-exported macro contract breaks; the resolver applies that cascade.
Such a mismatch blocks as `breakingSelectionUnderclassified`.

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
A normal/build dependency declaration or package-feature change is part of the published manifest and
always seeds a patch, even when it only fixes compilation under a workspace
feature configuration. Dev-dependency features do not.
Use `manifestDependencyScopes` as the authority for dependency scope rather than
inferring it from a `Cargo.toml` path or prose evidence.
The resolver requires normal/build dependency and package-feature changes to be
accepted, and gives a pure dev-dependency-only manifest edit exactly one valid
outcome: decline with `dev-dependency-only`.
A package promoted solely by an inherited `[workspace.dependencies]` change
carries no `modifiedFiles`, so read `externalDepChanges` for its diff: when the
change is not in `externalExposedDeps`, `runtime-manifest-change` is the reason;
when it is, only `breaking` is accepted.

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

### Behavior-fix evidence

`behavior-fix` is the only accepted reason that asserts observable behavior
changed, so the resolver requires it to be demonstrated rather than described.
Record `regressionEvidence` on the decision: one entry per probe, measured at
the release baseline and at the current revision.

```json
"regressionEvidence": [
  {
    "kind": "consumer-runtime",
    "probe": "cargo test -p cachet_tier --test eviction",
    "baseline": { "result": "fail", "revision": "<baselineRev>", "exitCode": 101 },
    "current":  { "result": "pass", "revision": "<HEAD>", "exitCode": 0 }
  }
]
```

`kind` is `consumer-runtime` (behavior a consumer observes at run time),
`consumer-compile` (a consumer fixture that must build), or `packaged-artifact`
(what the published `.crate` contains). Both sides must name the revision
measured, a `pass`/`fail` result, and the process exit code; `pass` pairs with
exit code 0 and `fail` with a non-zero code.

Only `fail` then `pass` on the same probe demonstrates a fix. Everything else
blocks the plan with zero releases:

| Observation | Ambiguity |
|---|---|
| No `regressionEvidence` recorded | `behaviorFixUndemonstrated` |
| pass then pass (behavior preserved) | `behaviorFixUndemonstrated` |
| fail then fail (still broken) | `behaviorFixUndemonstrated` |
| pass then fail (newly broken) | `behaviorFixUndemonstrated` |
| Missing result, revision, or exit code | `behaviorEvidenceInconclusive` |
| Exit code contradicts the result | `behaviorEvidenceInconclusive` |
| Both sides measured the same revision | `behaviorEvidenceInconclusive` |

Additional probes that did not move are allowed as long as one probe
demonstrates the fix. A refactor that preserves behavior cannot produce that
measurement, which is the point: classify it `internal-only` and decline.
Malformed entries -- prose instead of an object, a blank `probe`, an unknown
`kind` -- are rejected outright. Other reasons are unaffected;
`regressionEvidence` is ignored everywhere else.

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
      ],
      "compileEvidence": [
        {
          "ownerPackage": "templated_uri",
          "path": "crates/templated_uri/tests/ui/bad_placeholder.rs",
          "baseline": {
            "result": "fail",
            "revision": "7c185b447c5c1c94db36c6176d42093aa67b83a2",
            "exitCode": 101
          },
          "current": {
            "result": "fail",
            "revision": "HEAD",
            "exitCode": 101
          }
        }
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
- structured blocking when a changed compile fixture in a macro's review scope
  is unmeasured, inconclusive, or contradicts the declared verdict;
- structured blocking when a `behavior-fix` selection reason is not demonstrated
  by a probe that failed at the baseline and passes now;
- structured blocking when a breaking external dependency requirement change
  reaches a dependency the package's public API exposes;
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

The declared verdict is a checked assertion, not a declaration. The resolver
derives a floor from `compileEvidence` and blocks any verdict below it with
`macroVerdictUnderclassified`, reporting `declaredVerdict`, `derivedVerdict`,
and the `decidingFixtures`. A derived floor above `compatible` is also coupled
to selection: a package whose fixtures prove a break cannot be declined, and its
selection reason must be `breaking` (or, for a nonbreaking floor, one of
`breaking`, `nonbreaking-api`, `behavior-fix`). Every resolved plan echoes the
`derivedVerdict` next to the declared `verdict` in `macroContracts`.

A compatible proc-macro contract stays at the patch floor even when its
implementation dependency or exact pinned version crosses a Cargo major line.
An unresolved required contract returns `status: blocked` with sorted
`ambiguities`; no partial release plan may be applied. The compile-evidence
ambiguity kinds are:

| Kind | Meaning |
|---|---|
| `macroCompileFixtureUnevidenced` | A changed fixture has no `compileEvidence` entry |
| `macroCompileEvidenceInconclusive` | An entry lacks a pass/fail result, revision, or exit code |
| `macroVerdictUnderclassified` | The declared verdict is weaker than the derived floor |

Selection evidence blocks the same way. `behaviorFixUndemonstrated` means no
probe moved from failing to passing; `behaviorEvidenceInconclusive` means a
probe's measurement cannot be read or contradicts itself. Both name the missing
`requiredInput` and emit zero releases.

External exposure blocks the same way, without any recorded evidence to weigh:

| Kind | Meaning |
|---|---|
| `externalExposureUnderclassified` | A classification is weaker than the floor a breaking exposed external dependency change forces |
| `externalExposureUnderselected` | The selection reason for such a package is not `breaking` |
| `breakingSelectionUnderclassified` | A package selected as breaking has a weaker own objective classification |
| `ownClassificationUnsupported` | A previously released library is classified breaking/nonbreaking though only its doc comments, tests, or manifest changed and no exposed external dependency break forces it |

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

Each `macroContracts` entry echoes the reviewed `verdict` alongside the
`derivedVerdict` the resolver computed from `compileEvidence`:

```json
{
  "package": "ohno_macros",
  "verdict": "breaking",
  "derivedVerdict": "breaking",
  "reviewed": ["ohno", "ohno_macros"],
  "evidence": ["#[no_constructors] under #[ohno::error] is now rejected."]
}
```

Each `selectionDecisions` entry echoes the graded probes so the consensus review
compares measurements, not prose:

```json
{
  "package": "cachet_tier",
  "decision": "accept",
  "reason": "behavior-fix",
  "evidence": ["Eviction now honors the configured tier bound."],
  "regressionEvidence": [
    {
      "kind": "consumer-runtime",
      "probe": "cargo test -p cachet_tier --test eviction",
      "outcome": "fail->pass"
    }
  ]
}
```
