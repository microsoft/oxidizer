# `fetch_winhttp` agent guidelines

## Type documentation

Every nontrivial production type must have type-level documentation that
explains why the type exists and its role or invariant in the crate's design or
implementation. Readers must not need to reconstruct a type's purpose from its
call sites.

## Unwind safety

Assert the `UnwindSafe` and `RefUnwindSafe` status of every production type in
tests. Prefer the compiler-derived auto traits.

Add an explicit implementation only when a reviewed semantic invariant makes
the type unwind-safe despite a structurally conservative field. Document that
invariant immediately above each explicit implementation. When a type cannot
support either trait, add a negative assertion and document the reason in the
test.

## Test error handling

Tests use `.unwrap()` and `.unwrap_err()` instead of `.expect()` and
`.expect_err()`.
