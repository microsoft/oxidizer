# Releasing Oxidizer Packages

Prepare package versions with the
[`increment-versions` skill](../.github/skills/increment-versions/SKILL.md)
in **standalone mode**. This replaces the interactive PowerShell version planner,
its dependency-cascade implementation, and its changelog generation.

Version preparation runs **on demand**, when a maintainer asks for a release PR.
Ordinary code PRs do not need version changes or skill execution. The next release
assessment includes accumulated changes since each package's version anchor,
rather than only the latest code PR.

This adoption changes version preparation only. It does not install the toolkit's
publish-on-merge workflows, add `.cargo/release_plan.toml`, configure Trusted
Publishing, or change existing publication automation.

## Publication boundary

The standalone skill prepares a version-increment PR for normal review and
authorized merge. It does not authorize merging, publishing, tagging, or registry
administration. Existing internal Microsoft automation still tags and publishes
crates after version PRs merge; standalone mode does not disable that behavior.
The existing `publish-gh-release.yml` workflow and its supporting script remain
unchanged.

Do not run publication commands as part of version preparation. This adoption
does not establish a new publication procedure.

## Prerequisites

The command examples target PowerShell **7.6** or newer. The agent also needs
Git, the selected Cargo/Rust toolchain,
`cargo-release-plan`, and `cargo-semver-checks` on `PATH`. Install or upgrade
tools only with the applicable permission. Anvil manages the repository's Rust
verification tools; its setup does not install `cargo-release-plan`.

Select a published tool version supporting the schemas below and follow the
toolkit's [installation guide](https://folo-rs.github.io/folo/cargo-release-plan/integration/installation.html).
Do not assume the latest action release, the current online book, and the
current skill all select the same tool interface.

Check `cargo release-plan version` before assessment. The pinned upstream skill consumes
plan, report, and prepared schema **8**, semantic-decision schema **2**, and
compatibility schema **3**. Tool and skill package version numbers need not match,
but these schemas must. On a mismatch, stop and report it; update the wrapper's
pin and these expectations together in a reviewed change rather than bypassing
checks or silently following upstream `main`.

The local skill is a thin wrapper, not a vendored copy. Network access is required
to load the pinned upstream instructions and supporting files. If any resource
cannot be retrieved completely, planning must stop; a link alone does not load
a remote skill.

The standalone workflow does not require publication configuration, registry
bootstrap, or Trusted Publisher setup. API comparisons are separate evidence and
can still access published crates; standalone mode is not an offline guarantee.
Unavailable comparisons must be explained, not treated as proof of compatibility.

## Request version preparation

Run on a named feature branch. Explicitly select:

- The Cargo workspace to assess.
- A local ref or commit delimiting actual release history.
- For a stacked PR, the actual unmerged parent target ref, separately from release
  history. Otherwise, explicitly state that there is no unmerged parent.

These are invocation inputs, not defaults inferred from publishing configuration.
For example, if you choose the local `origin/main` ref:

> Run increment-versions in standalone mode for this workspace's Cargo.toml.
> Use my local origin/main ref as release history and no unmerged parent.
> Prepare the version-increment PR; do not configure or run publication.

That example does not fetch remote history. Explicitly request a remote refresh
if needed. For a stack, name the parent target in the request instead of using
the release-history ref as a substitute.

Existing tracked files can have staged or unstaged edits; a clean checkout or
prior source commit is not required. Newly created intended release inputs must
be tracked before assessment. Stage only those paths, preserving unrelated edits.

The wrapper loads the full pinned skill, its
[command reference](https://raw.githubusercontent.com/folo-rs/folo/fcc52639a902c8d8ad8000a58e0dfc39637bb6ac/.github/skills/increment-versions/commands.md),
decision guide, and license before planning. Follow all upstream stages.
Do not substitute a hand-assembled CLI sequence or create a dummy publication
configuration. In standalone mode, omit configured registry/publication gates
and `--config`, but retain compatibility and version-readiness verification.

## Assessment and review

The skill assesses complete released changes since each package's version anchor,
including inherited workspace metadata, pending increments, dependency effects,
re-exported APIs, and procedural macros. Choose semantic impacts (`breaking`,
`nonbreaking`, or `patch`); the tool resolves numeric versions, exact-requirement
version groups, and required manifest/lockfile changes.

Keep the existing package include allowlists and exact local dependency
requirements. The toolkit discovers groups from exact edges; do not add a
separate group table or widen requirements to avoid alignment. Workspace metadata
and packaged tests/docs count when included by the package's actual Cargo rules,
not by a production-source directory heuristic.

Keep Anvil's external-type checks: the planner consumes
`allowed_external_types` declarations but does not verify them against compiled
APIs. Compatibility findings alone cannot validate re-exported definitions or
generated macro behavior.

Published implementation crates are not automatically private APIs. Oxidizer
currently has no `package.metadata.release-plan.private-api` declarations; leave
their contracts conservative unless a separate review establishes that a package
has no independently supported API. Do not add that flag merely to bypass a
compatibility finding, and do not confuse it with `publish = false`.

Keep an adequate pending increment rather than increasing it solely because the
skill ran again. Assess public facades even when only their implementation
packages changed. Non-publishable packages receive version alignment only, not
semantic impacts or publication requests. A package with no comparison anchor
keeps its intended initial version unless group/dependency effects require
movement; do not invent an anchor or infer registry absence from Git history.

Keep all reports, compatibility output, plans, patches, decisions, and the
standalone input record outside the repository or under an already ignored
location. Use fresh output directories for repeated stages. Do not commit
evidence or edit tool-generated artifacts by hand.

Preparation may update `Cargo.lock`; inspect and retain the required change.
After preview and application, require fresh completed compatibility evidence
and a passing version-readiness check, then run the repository's applicable
Anvil checks. Do not claim readiness when tools or comparisons are incomplete.

Include a **Version/release plan** in the PR. Give one row per complete version
group or ungrouped package, with its anchor version, proposed version, semantic
impact, and substantive reason. Cover the union of resolved targets and pending
releases, including dependent movements and retained versions. Mark non-publishable
members **version alignment only, not published**. State explicitly when there is
nothing to release, and explain unavailable comparisons or other blockers.

Source, release-history, or parent-target changes require fresh assessment.
Recover by undoing only this run's verified versioning delta, preserving
independently authored changes. Do not reset the worktree to rerun planning.

## Changelogs and README files

The skill does not generate changelogs. Existing crate changelogs remain as
historical records; do not update them unless explicitly requested. Summarize
the current version decisions in the PR instead. The unchanged GitHub release
publisher can fall back to default release notes when a version has no changelog
entry.

Crate README files remain generated. Use `just anvil-readme --fix` rather than
editing them by hand.

## Upstream provenance and updates

The local `SKILL.md` provides Copilot discovery metadata and delegates to
[`folo-rs/folo` at `fcc52639a902c8d8ad8000a58e0dfc39637bb6ac`](https://github.com/folo-rs/folo/tree/fcc52639a902c8d8ad8000a58e0dfc39637bb6ac/.github/skills/increment-versions).
It requires the agent to fetch and read `SKILL.md`, `commands.md`,
`determining-level.md`, and the upstream MIT `LICENSE` from that same revision.
Upstream relative links resolve against the pinned remote directory. Any retained
downloaded copies must include the license and stay outside committed source.
Repository-specific policy remains in the wrapper, this guide, and `AGENTS.md`.

When updating, review the complete upstream directory from one known revision,
update every pinned URL in the wrapper and this guide, and update the documented
schema expectations together. Check the installed CLI's schemas before use.
Do not silently use an incompatible tool or switch modes to bypass a failed check.

After installation or an update, run `/skills reload` in Copilot CLI and
`/skills info increment-versions` to confirm discovery. Other Copilot hosts may
need a new session to discover the added skill.

See the toolkit's [local planning guide](https://folo-rs.github.io/folo/cargo-release-plan/integration/local-planning.html)
and [standalone guide](https://folo-rs.github.io/folo/cargo-release-plan/advanced/standalone.html).
