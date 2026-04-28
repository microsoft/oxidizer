# Bumping a Crate Version and Cascading to Dependents

You are a Rust developer working in the Oxidizer workspace. Your task is to bump the
version of a target crate and cascade an appropriate version bump to all of its
in-workspace dependents, respecting SemVer.

## Inputs

- **Target crate**: the crate whose version is being bumped (e.g. `tick`).
- **Bump kind**: `major`, `minor`, or `patch`.

## SemVer Rules

Cargo's resolver treats `0.x.y` versions specially: the `x` component is the
compatibility boundary (a bump of `x` is a breaking change), and `y` is the
non-breaking component. There is no separate "minor" slot in `0.x.y` — both
`minor` and `patch` bumps of a `0.x.y` crate map to bumping `y`. Apply the
following mapping consistently:

| Bump kind | `0.x.y` becomes      | `x.y.z` (x ≥ 1) becomes |
| --------- | -------------------- | ----------------------- |
| major     | `0.(x+1).0`          | `(x+1).0.0`             |
| minor     | `0.x.(y+1)` (= patch) | `x.(y+1).0`             |
| patch     | `0.x.(y+1)`          | `x.y.(z+1)`             |

For `0.x.y` crates, prefer using `patch` for any non-breaking change. The
`minor` row is provided so the cascade rules below behave uniformly when a
dependent is `0.x.y` but its target is `x.y.z` (or vice versa); it produces
the same result as `patch` for `0.x.y` crates.

Cascade rule for in-workspace dependents:

- **Major bump** of the target → **major bump** for every dependent (recursively).
- **Minor bump** of the target → **minor bump** for every dependent (recursively).
- **Patch bump** of the target → **patch bump** for every dependent (recursively).

A "dependent" here means any workspace crate that lists the target in its
`[dependencies]` (or `[build-dependencies]`) — **not** dev-only dependents. Crates
that consume the target only via `[dev-dependencies]` automatically pick up the new
workspace version and do not need their own version bumped, since their public API
is unchanged.

Apply the cascade transitively: after bumping a direct dependent, treat that
dependent as a new target and cascade to *its* dependents using the same bump kind.

## Instructions

0. **Discover the dependency graph**
    - List all workspace crates under `crates/`.
    - For each crate, inspect `Cargo.toml` and find which crates depend on the
      target (look for `<target> = { workspace = true }` or direct path/version
      entries) under `[dependencies]` / `[build-dependencies]`.
    - Build the transitive set of dependents to bump.
    - Skip crates that consume the target only under `[dev-dependencies]`.

1. **Bump the target crate**
    - Update `version = "..."` in `crates/<target>/Cargo.toml`.
    - Update the matching `<target> = { path = "...", ..., version = "..." }`
      entry in the workspace root `Cargo.toml`.

2. **Cascade to dependents**
    - For each direct and transitive dependent (excluding dev-only consumers):
      - Update `version = "..."` in `crates/<dependent>/Cargo.toml` per the
        SemVer table above.
      - Update the matching workspace entry in the root `Cargo.toml`.

3. **Update `CHANGELOG.md` for every bumped crate**
    - Inspect the crate's git history to write a meaningful entry — do **not**
      use a generic "bump version" message. From the repo root run:

      ```sh
      git log --since="<date-of-previous-release>" --pretty=format:"%h %s" -- crates/<crate>
      ```

      Use the previous release date from the top of the existing
      `CHANGELOG.md`. Discard commits already attributed to a prior released
      version, plus pure release/chore commits (e.g. README regeneration).
    - Add a new top-level section directly below the `# Changelog` header,
      using the format already present in the file:

      ```md
      ## [<new-version>] - <YYYY-MM-DD>

      - <category emoji + label>

        - <human-readable summary> ([#<pr>](https://github.com/microsoft/oxidizer/pull/<pr>))
      ```

    - Group bullets by category. Common categories (use the same emoji/label
      style as existing entries):
      - `✨ Features` — new functionality
      - `🐛 Bug Fixes` — bug fixes
      - `⚠️ Breaking` — breaking changes (mandatory for major bumps)
      - `🔧 Maintenance` — internal cleanups, dependency bumps with no
        user-visible effect
    - Each bullet should be a concise, user-facing description of the change
      (paraphrased from the PR title/description), with the PR link.
    - For **cascaded dependents** that have no other changes since their last
      release, a single `🔧 Maintenance` entry of the form
      `bump \`<target>\` to <new-version>` is acceptable. If the cascade is a
      major bump, place it under `⚠️ Breaking` instead.
    - Use the current date in `YYYY-MM-DD` form.
    - PR titles should still follow Conventional Commits, e.g.
      `chore(<crate>): bump to 0.x.y`.

4. **Verify**
    - Run `just format` to format any touched files.
    - Run `cargo check --workspace --all-features` (or `just package=<crate>
      check` per crate) to confirm the workspace still resolves and compiles.
    - Run `just readme` to refresh auto-generated crate READMEs.
    - Run `just spellcheck`.

5. **Summarize**
    - Report the old → new version for the target and every cascaded dependent.
    - Call out any crates that were intentionally **not** bumped (e.g. dev-only
      dependents) and the reason.

## Tips

- A "minor" bump on a `0.x.y` crate is **not** a breaking change in Cargo's
  resolver — `0.2.1` and `0.2.2` are compatible. A bump of the `x` component
  (e.g. `0.2.x` → `0.3.0`) **is** breaking and counts as major here.
- Always update **both** `crates/<name>/Cargo.toml` and the workspace root
  `Cargo.toml` entry — they must stay in lock-step or downstream crates will
  resolve to the wrong version.
- If the target crate is not referenced in the workspace root `Cargo.toml`
  (e.g. `testing_aids` has no `version`), only the crate-local `Cargo.toml`
  needs updating.
- When in doubt about whether a dependent is dev-only, search for the section
  header (`[dependencies]` vs `[dev-dependencies]`) immediately above the
  matching line.
