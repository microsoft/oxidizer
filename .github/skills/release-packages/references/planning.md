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

Each package fact contains:

`folder, name, version, published, procMacroOnly, hasLibraryTarget, deps,
exposedDeps, exposureUnknown, baselineSha, hasBaseline, everReleased, modified,
modifiedFileCount`.

- `deps` contains normalized non-dev dependency names.
- `exposedDeps` is the subset of those dependencies whose types are permitted in
  the package's public API.
- `exposureUnknown` is true for every unchecked target, such as a proc-macro,
  and for wildcard metadata. The resolver then assumes every dependency is
  exposed because metadata on unchecked targets is not CI-enforced.
- For ordinary libraries checked by `cargo check-external-types`, missing or
  empty metadata means no external dependency types are exposed.
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

The mechanical floor is patch and `manualReview` remains true. Review:

- exported macro names;
- accepted syntax;
- diagnostics;
- generated code and public types.

Record a stronger classification when the diff proves it.

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
- breaking propagation when a released dependency breaks consumers and the
  dependent exposes it;
- fixed-point propagation through chains and diamonds;
- proc-macro manual-review preservation;
- pin-versus-requirement rejection unless `force` is true;
- dependency-before-dependent ordering;
- deterministic, sorted cascade reasons.

An exposure edge is breaking when the dependency's version transition is breaking
under Cargo compatibility rules and the dependent lists it in `exposedDeps` or
has `exposureUnknown = true`. Therefore every `0.0.z` bump propagates as breaking
through exposure edges, even when its source classification was patch.

`force` never permits a downgrade or a pin equal to the current version. When it
keeps a pin below a computed requirement, the resolver records a warning and
retains the stronger effective change type for further cascades.

## Consensus gate

Before writing repository files:

1. Freeze facts, tokens, classifications, reviewed diffs, and `plan.json`.
2. Ask at least two additional model families to independently review the
   classifications and verify the plan follows from them.
3. Compare normalized tuples:
   `folder, to, changeType, source, manualReview, cascadeReasons`.
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
      "cascadeReasons": []
    }
  ],
  "warnings": [],
  "consensus": {
    "models": ["model-a", "model-b", "model-c"],
    "agreement": "unanimous"
  }
}
```
