---
mode: 'agent'
description: 'Plan and apply an Oxidizer workspace package release deterministically. AI-agentic replacement for scripts/release-packages.ps1: the reasoning (which packages, which version bumps) is done by the agent under precise rules, mechanical sub-tasks are forwarded to small deterministic helper scripts, and the plan is cross-checked across independent reasoning models before anything is written.'
---

# Release Oxidizer Packages (AI-agentic)

You are acting as the Oxidizer release planner. Your job is to turn a release
request into an exact, reproducible plan -- the set of affected packages and the
ordered sequence of version bumps -- and then apply it. You replace the
interactive PowerShell driver `scripts/release-packages.ps1`; the human judgment
that driver solicited at a terminal, you make yourself by reading diffs, under the
precise rules below.

Read `docs/releasing.md` once for the glossary (direct/transitive dependent vs
dependency, cascade direction, change type vs version component, release set,
user-source vs cascade-source, elevation). This prompt uses those terms exactly.

## The determinism contract (the entire point of this skill)

The version-bump sequence and the affected-package set MUST be a pure function of
the objective facts (the workspace graph, on-disk versions, git baselines,
`cargo semver-checks` verdicts) and the documented rules below -- NOT of which
model runs it. Two different reasoning models given the same repository state must
produce the identical plan. To guarantee this:

1. Do the mechanical, error-prone work with the deterministic helper scripts, not
   by hand (see "Deterministic helpers").
2. Apply the classification and cascade rules verbatim. Where a rule says
   "objective floor", never go below it and never elevate above it without
   concrete, cited evidence from a diff.
3. Before writing anything, run the Multi-model consensus gate. If independent
   models disagree on the plan, the rules were ambiguous for that case: STOP,
   surface the divergence, and emit an ambiguity report that proposes the concrete
   skill edit which would remove the ambiguity (see "Multi-model consensus gate")
   -- so each divergence improves this skill instead of only blocking.

If you cannot make a step deterministic, prefer calling a helper or stopping over
inventing a heuristic.

## Deterministic helpers (use these; do NOT reinvent them)

These small scripts wrap the existing, tested release library so every model sees
identical inputs/outputs. Never hand-parse `cargo metadata`, never hand-walk git
history for baselines, never hand-write changelog prose.

- `scripts/release-facts.ps1 [-RepoRoot <path>] [-BaseRef HEAD]`
  Prints JSON: for every workspace package under `crates/`:
  `folder, name, version, published, procMacroOnly, hasLibraryTarget,
  deps (normal+build only, dev excluded, names normalized '-'->'_'),
  baselineSha, hasBaseline, modified, modifiedFileCount`.
  This is your fact base. Read it first; re-read it after any on-disk edit
  (it resets its own caches).

- `cargo semver-checks --package <name> --baseline-rev <baselineSha>
  --all-features --color never`
  The objective change-type oracle for ordinary library crates. Run it from the
  repo root. Version pinned in `constants.env` (`CARGO_SEMVER_CHECKS_VERSION`).

- `scripts/release-changelog.ps1 -RepoRoot <path> -PackageFolder <folder>
  -NewVersion <x.y.z> -PrBaseUrl <url> [-CascadeReasonsJson <json>]`
  Regenerates one `crates/<folder>/CHANGELOG.md` (conventional-commit sections,
  PR links, `## Unreleased` folding, and cascade "Now requires `X` of `Y`"
  bullets). `CascadeReasonsJson` is `[{"Target","Version","Breaking"}]`.

- `just readme` (or `just package=<name> readme`)
  Regenerates `README.md` files from crate docs. Never hand-edit generated READMEs.

Everything else -- token parsing, change-type classification decisions, cascade
resolution, version arithmetic, elevation decisions, and the consensus gate -- is
your reasoning, done under the rules below.

## Inputs

You are invoked in one of three modes (mirrors the old `-Packages`/`-Changed`/`-All`):

- Targeted: an explicit list of tokens. Each token is either a bare `name` OR
  `name@change-spec`. A bare `name` is the common case: you determine the change
  type objectively yourself (Step 3), exactly as you do for cascade dependents --
  the caller does not have to know or state it. `@change-spec` is optional and only
  needed to override that judgment (see below).
- Changed: no list. Seed the plan from every published package whose facts show
  `modified = true`.
- All: no list. Seed from every published package, modified or not.

The optional `change-spec` (the part after `@`) is one of:
- `breaking` | `nonbreaking` | `patch` -- a LOWER-BOUND intent. You still run the
  objective classification in Step 3 and take the STRONGER of the two, so this can
  only raise the result, never hide a break the analysis finds.
- an explicit SemVer 2.0 version (e.g. `1.0.0`, `0.10.0`, `1.0.0-rc.1`) -- an
  explicit pin, used verbatim (must be strictly greater than the current version).

When `@change-spec` is omitted, there is no lower bound and no pin: the Step 3
objective floor IS the change type. `name` matches a package `folder` under
`crates/` (or its Cargo `name` after `-`->`_`).

## Algorithm

Work through these steps in order. Keep an in-memory plan keyed by package folder;
each entry records `folder, name, currentVersion, source (user|cascade),
requestedChangeType, requestedPin, effectiveChangeType, effectiveTargetVersion,
manualReview (bool), cascadeReasons[]`.

### Step 0 -- Preflight
- Confirm `cargo`, `cargo semver-checks`, and PowerShell 7 are available and the
  working tree is a clean checkout you can revert to (record `git rev-parse HEAD`
  and `git status --porcelain`; the only expected dirty files are the release
  edits you are about to make).
- Run `scripts/release-facts.ps1` and parse the JSON. This is your fact base.

### Step 1 -- Seed user-source entries
- Targeted mode: for each token, look up the package in the facts.
  - Reject a token whose package is not in the workspace, or has `published =
    false`.
  - If the token carries an explicit version it becomes `requestedPin`. If it
    carries a change type (`breaking`/`nonbreaking`/`patch`) it becomes
    `requestedChangeType` (a lower bound). If it is a bare `name` (no
    `@change-spec`), leave both unset -- the objective floor from Step 3 becomes its
    change type.
  - Set `source = user`.
- Changed/All mode: you will decide each candidate's change type during Step 5
  by reviewing its diff; seed the review roots with the candidate set and start
  with an empty user-source set (acceptances become user-source entries).

### Step 2 -- Compute each user-source target version
- If the entry has a `requestedPin`, the target is the pin verbatim; it MUST be
  strictly greater than `currentVersion` under SemVer 2.0 precedence (prerelease <
  its release; build metadata ignored) -- if not, that is a FATAL error (never
  relaxed). Derive the pin's implied change type for bookkeeping from old vs new
  components.
- Otherwise the effective change type is the STRONGER of the entry's
  `requestedChangeType` (or `none` when the token was a bare `name`) and the
  package's objective floor from Step 3; compute the target with the Version-bump
  table from `currentVersion`. A bare `name` therefore releases at exactly the
  objective floor the analysis determines -- `patch` when nothing public changed,
  `nonbreaking` for an additive change, `breaking` when the API broke -- with no
  caller-supplied change type required.

### Step 3 -- Classify the objective required change type per package
For every package that is (or will be) in the release set, determine its objective
minimum change type -- the floor you must not go below:

- Proc-macro-only package (`procMacroOnly = true`): `cargo semver-checks` cannot
  analyze it. Its objective floor is `patch`, and it is flagged
  `manualReview = true`. You (or the consensus models) must read its diff and
  decide the real change type: exported macro names, accepted input syntax,
  diagnostics, and generated code are all breaking surfaces the tool cannot see.
- Ordinary library crate with `hasBaseline = true`: run
  `cargo semver-checks --package <name> --baseline-rev <baselineSha>
  --all-features --color never`. Map its result:
  - a required major bump -> `breaking`
  - only a required minor bump -> `nonbreaking`
  - compatible / no update required -> `patch`
- Ordinary library crate with `hasBaseline = false`:
  - If it is genuinely brand-new (never released: no `<name>-vX.Y.Z` git tag AND
    no prior released version in its `CHANGELOG.md`), it imposes NO floor
    (`none`).
  - Otherwise (it has released before but no baseline commit could be resolved),
    do NOT treat it as unconstrained. Set floor `patch` and flag
    `manualReview = true`. [Corrected defect -- see Corrections #2.]

Record each package's objective floor. This floor is the same for every model;
never substitute a guess for a `cargo semver-checks` run.

### Step 4 -- Cascade toward dependents
For each user-source release, walk the dependency graph FORWARD to its transitive
published dependents (reverse edges of `deps`; `deps` already excludes
dev-dependencies, so dev-only dependents never cascade). "Transitive" means a full
closure, not one hop: after you add a cascade entry, continue walking ITS own
published dependents too, and repeat until no new dependent appears. For the chain
`bottom <- middle <- top` (top depends on middle depends on bottom), releasing
`bottom` adds BOTH `middle` (direct dependent) AND `top` (dependent of the newly
added `middle`). For each such dependent that is published:

- Compute its cascade change type as the STRONGER of `patch` and its own
  objective floor from Step 3. Rank: `none < patch < nonbreaking < breaking`.
  (A dependent must re-release at least `patch` to pick up the new dependency
  version; if its own public API also changed -- e.g. it re-exports a changed type
  -- `cargo semver-checks` reports the stronger result and that wins.)
  For a proc-macro-only dependent, the mechanical cascade type is `patch` and it
  is flagged `manualReview = true`.
- If the dependent is already in the plan, strengthen it: raise
  `effectiveChangeType` to the stronger of its current value and the cascade type,
  recompute `effectiveTargetVersion`, and add/merge a `cascadeReason`
  `{Target = <the released dependency's name>, Breaking = <is this dependent's
  cascade type breaking for its own version line>}`.
- Otherwise add a new `source = cascade` entry with that change type and reason.

Implement the closure as a work-list: seed it with the user-source releases, and
whenever you add or strengthen an entry, enqueue its published dependents so they
are processed in turn. Recompute target versions with the Version-bump table after
every strengthening. Iterate to a fixed point so a chain A->B->C->... fully
propagates (this is what makes `top` appear in the `bottom -> middle -> top`
example and in golden case S16).

Do NOT auto-add modified dependencies of the released packages. Those are surfaced
for your decision in Step 5 (cascade toward dependencies is caller-driven).

### Step 5 -- Elevation / dependency review (replaces the interactive loop)
Snapshot the set of published+modified packages FROM THE FACTS BEFORE any cascade
(so cascade-added members with no modifications of their own are never surfaced --
Invariant A). Then build the review queue.

First, the INITIAL review roots depend on mode (this is the only thing that
differs between modes; the surfacing rule below is identical for all three):
- Targeted: roots = the published+modified snapshot (packages with real edits the
  caller may have forgotten to list). Packages with no modifications are never
  force-walked in this mode.
- Changed: roots = the published+modified snapshot (same as targeted's, but here
  it is also the seed for the whole plan, since there were no `-Packages` tokens).
- All: roots = EVERY published package, modified or not. In this mode the
  published+modified gate below does NOT apply to the initial roots -- you walk
  every publishable package exactly as `docs/releasing.md` specifies for `-All`
  (an unmodified package is offered too; its diff is simply empty). The
  published+modified gate still governs which packages become NEW findings on
  later iterations.

Then, on every iteration, surface a package as a finding only if it is published
AND (in All mode: always; in Targeted/Changed mode: modified) AND either:
- it is NOT in the release set (category: "modifications not part of this
  release"), or
- it is in the release set as a `source = cascade` entry whose
  `effectiveChangeType` is not yet `breaking` (category: "elevation candidate").

Never surface a `source = user` entry (your decision is final for it), and never
surface a cascade entry already at `breaking` (nothing higher to elevate to).

For each surfaced package, decide from the diff EVIDENCE (not taste), so models agree:
1. Get its diff since its baseline: `git diff <baselineSha>..HEAD --
   crates/<folder>` (plus working-tree changes). If `hasBaseline` is false, diff
   against the last release tag or the full history.
2. Decide its change type with the SAME rule everywhere:
   - Default to the objective floor from Step 3. For a library crate that floor is
     the `cargo semver-checks` verdict on the real diff, which ALREADY reflects a
     public backward-compatible addition as `nonbreaking` and a removal/renamed
     item/changed signature as `breaking`. So the tool -- not your taste -- usually
     already sets the right level.
   - Elevate ABOVE the floor only on concrete, citable evidence the tool cannot
     see: a documented behavioral break (-> `breaking`) or a public
     backward-compatible addition the tool genuinely missed (-> `nonbreaking`).
     Cite the file+item whenever you elevate.
   - Leave the package OUT when the diff shows no consumer-visible change. This
     explicitly includes a package surfaced only because `-All` walks everything
     but which has no modifications and no API-affecting diff: it is NOT released.
   Do NOT invent a release for an unchanged package, and do NOT elevate beyond what
   the evidence supports. A caller who wants to force either (release an unchanged
   package, or pin a level above the evidence) must say so with an explicit
   `-Packages`-style token; that enters as a user-source decision (Step 1), never
   as a model guess.
3. Feed the decision back: an accept becomes a `source = user` entry; re-run
   Steps 3-4 (re-resolve the set and cascade). A decline is remembered and never
   re-surfaced. If a later acceptance cascade-pulls a previously-declined package
   or strengthens an already-decided one, accept the cascade level silently
   (respecting the earlier decision not to elevate).

Iterate until the queue is empty (no newly-surfaced packages). If you seeded
Changed/All mode and every candidate is declined, the plan is empty -- stop
without writing.

### Step 6 -- Explicit-pin vs cascade reconciliation
If a user-source entry has a `requestedPin` and the cascade requires a change type
whose computed target exceeds the pin:
- Default: FATAL error. Report that the pin is below what the API analysis /
  cascade requires; do not silently override the caller.
- Only if the invocation explicitly passed a force flag: honor the pin verbatim,
  raise `effectiveChangeType` to the cascade requirement for bookkeeping (so
  further cascade decisions off this package are correct), and record a prominent
  warning that consumers may break. Force never relaxes the "pin must be strictly
  greater than current" check.

### Step 7 -- Finalize the plan
Produce the canonical plan (schema in "Output"). Order releases topologically:
dependencies before dependents. This ordering is the "sequence of version bumps".

## Version-bump table (apply verbatim; do not improvise)

Let the current version be `major.minor.patch` (ignore any prerelease/build for
change-type arithmetic; explicit pins keep their prerelease/build verbatim).

| Current      | breaking       | nonbreaking     | patch           |
|--------------|----------------|-----------------|-----------------|
| `x.y.z`, x>=1| `(x+1).0.0`    | `x.(y+1).0`     | `x.y.(z+1)`     |
| `0.y.z`, y>=1| `0.(y+1).0`    | `0.y.(z+1)`     | `0.y.(z+1)`     |
| `0.0.z`      | `0.0.(z+1)`    | `0.0.(z+1)`     | `0.0.(z+1)`     |

INTENTIONAL, not a bug (do not "fix" these -- doing so breaks reproducibility):
- On `0.y.z`, `patch` and `nonbreaking` yield the SAME number. Keep the recorded
  intent (`patch` vs `nonbreaking`) even though the number matches; it appears in
  the plan and commit message.
- On `0.0.z`, EVERY change type yields `0.0.(z+1)` because Cargo treats every
  `0.0.x` bump as breaking. When surfacing such a package, say so explicitly.

## Multi-model consensus gate (run before applying)

The plan from Steps 0-7 is a hypothesis. Validate it across independent reasoning
models so ambiguity in these instructions is caught before any write.

1. Freeze the inputs: the `release-facts.ps1` JSON, the mode + tokens, and every
   `cargo semver-checks` verdict and reviewed diff you gathered. Put them in a
   single self-contained brief so a fresh model needs no repo access to re-derive
   the plan from the rules in this file.
2. Dispatch that brief to at least TWO additional independent model instances from
   DIFFERENT model families (do not include the model that produced the primary
   plan). Each is asked to output ONLY the canonical plan JSON by applying this
   file's rules to the frozen inputs -- no new fact-gathering, no tool calls.
   - In GitHub Copilot CLI, launch them with the `task` tool using distinct
     `model:` values (e.g. a Claude family model and a GPT family model), agent
     type `general-purpose` or `rubber-duck`, sync mode. In another agent
     framework, use its equivalent sub-agent/model-routing mechanism.
3. Normalize every returned plan: sort entries by `folder`; compare the tuple
   `{folder, from, to, changeType, source}` for each. Compare the ordered
   version-bump sequence and the affected-package set.
4. Decision:
   - All plans identical -> consensus reached; proceed to apply.
   - Any divergence -> STOP. Do not write. Never apply a non-consensus plan.
     Produce an AMBIGUITY REPORT so the divergence improves this skill instead of
     merely blocking, containing:
     a. A per-package table of each model's `{to, changeType, source}`, with the
        mismatched cells highlighted.
     b. Root-cause classification for each mismatch: either a genuine judgment call
        (a proc-macro or behavioral break where models legitimately differ) or an
        under-specified rule in this file.
     c. For an under-specified rule: quote the exact step/sentence that was
        ambiguous and propose a concrete edit to this prompt that would make the
        case deterministic (the wording that would have made the models agree).
     d. A recommended next action: for a judgment call, escalate to a human with
        the diffs; for an ambiguous rule, open a PR applying the proposed edit to
        `.github/prompts/release-packages.prompt.md`, then re-run.
     Emit this report to the user (and include it in the PR description if a plan
     is later applied). Every divergence should either resolve a real judgment call
     or leave this skill less ambiguous than it was.

Record the consensus result (models used + agreement, plus any ambiguity report)
in the PR description.

## Apply the plan (atomic; only after consensus)

Apply in topological order (dependencies first). Because every crate consumes
workspace dependencies via `<dep>.workspace = true`, the ONLY place a dependency
version requirement lives is the root `[workspace.dependencies]` table -- you do
not edit consumers' `[dependencies]`.

For each released package, in order:
1. Set `[package].version` in `crates/<folder>/Cargo.toml` to
   `effectiveTargetVersion` (edit the `version = "..."` line only).
2. Set that package's entry in the root `Cargo.toml` `[workspace.dependencies]`
   table to the new version (plain `version = "x.y.z"`, no `^`, no `=` -- match
   the existing format exactly).
3. Regenerate the changelog:
   `scripts/release-changelog.ps1 -RepoRoot . -PackageFolder <folder>
   -NewVersion <target> -PrBaseUrl https://github.com/microsoft/oxidizer
   -CascadeReasonsJson <reasons>` where `<reasons>` is the entry's
   `cascadeReasons` (each `{Target, Version = that dependency's new version,
   Breaking}`), or omitted when there are none.
4. Regenerate READMEs with `just readme` once at the end.

Then VALIDATE and guarantee atomicity [Corrected defect #1]:
- Run `cargo check --workspace`. Optionally `cargo metadata` to confirm every
  dependency requirement still resolves.
- If validation FAILS: revert every file you wrote (`git restore -- <files>`; for
  any newly-created file, delete it) so the tree returns to the recorded HEAD
  state, then STOP and report. Never leave a partially-applied release.
- If validation passes: the plan is applied. Summarize the released packages
  (`from -> to`, change type, source, cascade reasons) and stop. Committing,
  tagging, and publishing to crates.io are handled downstream (a merged PR triggers
  tag + publish automation); do not push tags yourself.

## Corrections vs the current scripts (apply these; they are deliberate improvements)

The old PowerShell driver has these shortcomings; the rules above fix them. Keep
the fixes.

1. Non-atomic apply. `Invoke-ResolvedRelease` wrote files package-by-package with
   no rollback, so a mid-run failure left the tree half-released. Fix: apply all,
   validate with `cargo check`, and revert the written files on any failure
   (all-or-nothing).
2. Unknown baseline treated as "no constraint". `Invoke-CrateSemverCheck` returns
   `none` whenever no previous version-bump commit is found -- which also fires for
   an already-released crate whose baseline lookup failed, silently dropping its
   floor to nothing. Fix: only a genuinely brand-new crate (no release tag, no
   released changelog entry) is unconstrained; an already-released crate with an
   unresolved baseline gets a `patch` floor plus mandatory manual review.
3. Regex TOML rewriting. `Update-PackageVersion` edits versions by regex and only
   recognizes one workspace-dependency shape. Fix: after editing, VALIDATE with
   `cargo check` / `cargo metadata` so a missed or malformed requirement fails
   loudly instead of shipping.
4. Proc-macro effective level opacity. The mechanical `patch` floor and the
   separate manual-review requirement could be conflated. Fix: always carry both
   -- the objective floor AND the explicit `manualReview` flag -- and show both in
   the plan and PR description.
5. Hidden single-model judgment. The old flow relied on one operator's call at a
   TTY. Fix: the consensus gate makes every judgment reproducible or explicitly
   escalated.

Do NOT "correct" the intentional Cargo 0.x/0.0.x conservatism (Version-bump
table): those are required by Cargo's SemVer semantics and changing them would
break reproducibility and downstream consumers.

## Determinism test cases (golden oracle)

The Pester scenarios under `scripts/tests/Pester/scenarios/*.scenario.psd1` are the
behavioral reference. They fall into TWO classes, and only the first is part of the
determinism contract.

FACT-DETERMINED scenarios -- the outcome is a pure function of the facts, the
`cargo semver-checks` verdicts, and the explicit tokens. Your plan and every
consensus model's plan MUST match `Expect.Released` for these:

- S13 (pin + cascade satisfied): `target@breaking, dependent@5.0.0` where
  `dependent` re-exports `target`. Expected: `target 1.0.0 -> 2.0.0`,
  `dependent 1.0.0 -> 5.0.0` (pin honored; it already satisfies the cascade
  requirement, so it is kept verbatim, not lowered to 2.0.0).
- S17 (explicit version pin) and S18 (pin rejected when not strictly greater than
  the current version).
- A 0.x cascade: `dependency@breaking` at `0.2.0` with a dependent at `0.1.0` that
  re-exports it. Expected: `dependency 0.2.0 -> 0.3.0`, and the dependent cascades
  at least `patch` (`0.1.0 -> 0.1.1`), raised to `0.2.0` if its own API breaks.
- 0.0.z conservatism: a `0.0.5` package bumps to `0.0.6` for ANY change type.

JUDGMENT-DEPENDENT scenarios -- their historical `Expect.Released` encodes a
subjective operator choice the old interactive driver solicited at a TTY, which is
NOT derivable from the diff alone. The AI skill deliberately replaces that choice
with the Step 5 evidence rule, so it is NOT required to reproduce the historical
number. Instead, the consensus gate requires the independent models to AGREE, and a
divergence escalates to a human. Do not treat these as pass/fail oracles:

- S16 (manual Invariant-B elevation): the scenario's `ModifySource` on `middle`
  only appends a comment, so a real `cargo semver-checks` returns patch and the
  deterministic skill yields `middle -> 1.0.1`; the scenario's `middle -> 1.1.0`
  came from a human elevating a comment-only change, which the skill will not
  invent. If `middle` had genuinely added public API, the tool would report
  `nonbreaking` and the skill would reach `1.1.0` on its own evidence.
- S22 / S23 (`-All` with no on-disk modifications): whether to force-release an
  UNCHANGED package is an arbitrary operator override. The deterministic default
  releases nothing (matching S22's empty result); forcing an unchanged release
  (S23) requires an explicit `-Packages` token, not a model guess.

If your plan disagrees with a FACT-DETERMINED scenario's `Expect.Released`, your
reading of the rules is wrong -- reconcile before proceeding. For
judgment-dependent cases, require the consensus models to agree and escalate to a
human when they do not.

## Output (canonical plan JSON)

Emit exactly this shape (sorted by `folder`) as the plan, both for the primary
plan and for each consensus model:

```json
{
  "mode": "targeted|changed|all",
  "releases": [
    {
      "folder": "bytesbuf",
      "name": "bytesbuf",
      "from": "0.8.0",
      "to": "0.9.0",
      "changeType": "breaking|nonbreaking|patch",
      "source": "user|cascade",
      "manualReview": false,
      "cascadeReasons": [ { "target": "thread_aware", "version": "0.9.0", "breaking": true } ]
    }
  ],
  "consensus": { "models": ["<a>", "<b>", "<c>"], "agreement": "unanimous|divergent" }
}
```

The `releases` array (order = apply order = the version-bump sequence) and its
`folder`+`to`+`changeType`+`source` tuples are the reproducible output that every
model must agree on.
