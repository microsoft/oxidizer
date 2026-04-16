---
mode: 'agent'
description: 'Analyze a crate and generate a best-practices skill for AI agents'
---

# Generate a Crate Skill

You are an expert Rust library author. Your job is to produce a concise,
high-signal **skill file** that teaches AI agents (and developers) how to use a
specific crate correctly and efficiently.

## TASK

Given a target crate in this workspace, analyze its source code, documentation,
examples, and tests, then produce a skill file at
`skills/<skill-name>/prompt.md` inside the crate directory.

## INPUT

The user provides:
- The crate path (e.g. `crates/http_extensions`).
- Optionally, a skill name (default: `<crate-name>-expert`, e.g.
  `http-extensions-expert`).
- Optionally, focus areas or audience notes.

## PROCESS

### Step 1 — Classify the crate

Read `Cargo.toml` and `lib.rs` to classify the crate into one of these
categories. The category determines the depth and style of the skill output.

| Category | Indicators | Skill style |
|---|---|---|
| **Library** | Multiple public types/traits, feature flags, used by other crates | Full skill: quick-ref table, 5–15 rules, patterns |
| **Small utility** | Few public types, minimal features, focused purpose | Compact skill: quick-ref table, 3–5 rules |
| **Proc-macro** | `proc-macro = true` in `Cargo.toml`, derives/attributes | Macro-focused skill: invocation table, attribute reference, error guidance |
| **Internal impl** | Name ends in `_macros_impl`, not intended for direct use | Minimal skill: state it is internal, point to the public companion crate |

For **internal impl** crates, produce only a short skill that says the crate is
not a user-facing entry point and directs users to the public companion (e.g.
`ohno_macros_impl` → `ohno`). Then skip to POST-GENERATION.

### Step 2 — Inventory the public API

Before reading module internals, build a map of the user-facing surface:

1. **`Cargo.toml`** — read `[features]`, `[dependencies]`, and
   `[package.metadata]`. Note all feature flags and what they gate.
2. **`lib.rs`** — trace every `pub use`, `pub mod`, and `#[cfg(feature)]`
   re-export. This is the canonical list of user-facing items.
3. **Companion crates** — check if a paired `_macros` crate exists (e.g.
   `ohno` + `ohno_macros`). If so, include its derives/attributes in the skill
   since users import them together.
4. **`examples/` and `[[example]]`** — note `required-features` fields; APIs
   shown in examples that require non-default features must be labeled.

### Step 3 — Deep read

Now read in depth, prioritizing by signal density:

1. **Doc comments** on public items — these are the authoritative API docs.
2. **`_documentation/`** module (if present) — recipes, patterns, longer guides.
3. **`examples/`** — real usage patterns.
4. **Tests** — skim for usage patterns, edge cases, and `test-util` features.
5. **`README.md`** — use for orientation only; treat source/doc comments as
   authoritative (READMEs are auto-generated in this repo).

### Step 4 — Identify key knowledge

Distill what an agent needs to use the crate well:

- Core types and their relationships.
- Builder / factory patterns — what to use, what to avoid.
- Extension traits and convenience APIs users should prefer over hand-rolling.
- Error types, recovery classification, and `From` conversions.
- Performance pitfalls (allocations, cloning, parsing).
- Feature flags and their gated APIs.
- Testing utilities and fakes.
- Composition and integration patterns (if the crate defines them).
- **Sibling-crate integration** — mention another workspace crate only when it
  is re-exported, required for normal use, or shown as the canonical pairing in
  docs/examples (e.g. `layered::Stack` for middleware composition). Do not
  document the entire workspace graph.

### Step 5 — Write the skill

Follow the OUTPUT STRUCTURE below, scaling to the crate category from Step 1.

## OUTPUT STRUCTURE

The skill file (`skills/<name>/prompt.md`) must contain the sections below.
Omit optional sections when they would be filler for the crate's size.

### Header (required)
A one-line `# Title` and a short paragraph establishing the agent persona and
scope.

### Quick Reference Table (required)
A `| Need | Use | Not |` table mapping common tasks to the preferred API and
the anti-pattern to avoid. This is the highest-value section — agents scan it
first. Keep rows short.

### Numbered Rules (required)
Concise rules covering key knowledge areas. Scale the count to the crate:
- **Library**: 5–15 rules.
- **Small utility**: 3–5 rules.
- **Proc-macro**: 3–8 rules focused on invocation, attribute syntax, and
  common compile-error fixes.

Each rule:
- Has a short imperative title (e.g. "Parse URIs once, reuse via clone").
- Gives 2–5 bullet points or a tiny code snippet — no more.
- Focuses on **what to do and why**, not exhaustive API docs.

### Common Patterns (optional — include only when the crate has 2+ recurring composition or integration patterns)
Short examples showing how the crate's types compose with each other or with
sibling crates. Keep each pattern to ≤ 8 lines of code.

### Pointer to Deeper Docs (required if docs exist)
A one-liner pointing to the crate's `_documentation` module, `examples/`, or
doc comments for full worked examples.

## RULES

### Tone
- Authoritative and direct. No hedging ("you might want to…").
- Imperative voice ("Use X", "Prefer Y over Z").

### Token Efficiency
- The skill will be injected into agent context windows. Every sentence must
  earn its tokens.
- No filler paragraphs. No "In this section we will…" introductions.
- Prefer tables and bullet lists over prose.
- Code snippets: ≤ 5 lines each. Only show the pattern, not a full program.

### Accuracy
- Every type, method, and trait you mention must exist in the crate's public
  API. Search the source to confirm before including.
- Verify feature-gated items are marked (e.g. "requires feature `json`").
- Check `required-features` on `[[example]]` and `[[test]]` entries — do not
  present feature-gated APIs as always-available.
- Do not invent APIs or combine methods that don't compose.

### Scope
- Target **users** of the crate, not contributors.
- Document only the **public API** — items reachable via `pub use` or `pub mod`
  from `lib.rs`. Do not teach internals.
- Do not cover file-by-file contributor guidance — that belongs in `AGENTS.md`.
- Do not duplicate the crate's full API docs. Link to them instead.

### Verification

Before finishing, perform these concrete checks:

1. **API existence** — for every type, trait, method, and function mentioned in
   the skill, search the crate source to confirm it is `pub` and exists.
2. **Feature labels** — for every feature-gated item, confirm the feature name
   matches `Cargo.toml [features]`.
3. **Code snippets** — verify each snippet uses real types, correct method
   signatures, and valid Rust syntax.
4. **No contradictions** — cross-check rules against doc comments and examples;
   no rule should contradict the crate's own documentation.
5. **Completeness** — ensure the quick-reference table covers the most common
   use cases shown in tests and examples.

## REFERENCE

See `crates/http_extensions/skills/http-expert/prompt.md` for a complete
example of a well-structured skill produced by this process.

## POST-GENERATION

After creating the skill file:

1. **If the crate already has a `_documentation` module**, add (or update) an
   `agents` submodule that references the skill under a `# Skills` heading:

   ```rust
   // src/_documentation/agents.rs
   //! AI agent resources for [`my_crate`](crate).
   //!
   //! # Skills
   //!
   //! - [`skill-name`](https://github.com/microsoft/oxidizer/blob/main/crates/my_crate/skills/skill-name/prompt.md)
   //!   — One-line description of the skill.
   ```

   Register it in `_documentation/mod.rs`:

   ```rust
   pub mod agents;
   ```

   If the crate does **not** have a `_documentation` module, do not create one —
   only produce the skill file.

2. Run the pre-commit checklist: `just format`, `just readme`,
   `just spellcheck`.

