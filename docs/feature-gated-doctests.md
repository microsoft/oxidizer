# Feature-gated doctests

Doctests that reference items behind a non-default Cargo feature (e.g.
`test-util`, `retry`, `timeout`) fail to compile when only default features
are enabled. The repository runs doctests both with default features and
with `--all-features`, so the same source must work in both configurations.

The fix wraps the body in hidden `#[cfg(...)]` lines that exclude the
feature-dependent code when the feature is not enabled. The `#` prefix hides
the wrapper from rendered documentation but rustdoc still compiles those
lines.

## Pattern A — implicit `main` with `?`

When the body uses the `?` operator (and ends with a hidden
`# Ok::<_, _>(())` for type inference), rustdoc auto-injects a
`Result`-returning `fn main`. We can't simply cfg-gate a `{ ... }` block
inside that auto-`main`: when the feature is off, the auto-`main` still has
return type `Result<(), E>` but the cfg-gated `Ok(())` is gone, so the
function falls through without a value and fails to compile.

We replace rustdoc's auto-wrapping with our own non-`Result`-returning
`fn main`, and absorb `?` via an IIFE inside a cfg-gated block:

```text
/// ```
/// # fn main() {
/// # #[cfg(feature = "test-util")] {
/// # (|| {
/// // ... visible body that uses `?` ...
/// # Ok::<(), MyError>(())
/// # })().unwrap();
/// # }
/// # }
/// ```
```

When the feature is off the cfg-gated block compiles to nothing, and the
empty `fn main` is a valid no-op test.

## Pattern C — no `?`, no user `main`

A single cfg-gated `{ ... }` block inside a hidden `fn main` is enough:

```text
/// ```
/// # fn main() {
/// # #[cfg(feature = "X")] {
/// // ... body ...
/// # }
/// # }
/// ```
```

## Pattern B — the doctest already declares `fn main`

When the doctest defines `fn main` itself (often via `#[tokio::main]`), we
can't introduce a second `fn main`, and we can't put a `#[tokio::main]`
attribute inside a `{ ... }` block. Result-returning `async fn main` has the
same return-type problem as Pattern A.

We therefore use a **two-`main` shim**: a stub for the feature-off case and
the user's real `main` for the feature-on case:

```text
/// ```
/// // top-level items that don't reference the feature are left alone
/// use my_crate::PublicType;
///
/// fn helper() -> PublicType { /* ... */ }
///
/// # #[cfg(not(feature = "test-util"))] fn main() {}
/// # #[cfg(feature = "test-util")]
/// # #[tokio::main]
/// # async fn main() {
/// #     // body that uses feature-only symbols (new_fake, FakeHandler, ...)
/// # }
/// ```
```

Top-level items that don't touch feature-gated symbols stay ungated.
Only `use` statements that literally import a feature-only symbol
(`FakeHandler`, `FakeRead`, `FakeWrite`, `FakeServer`, `Null`) need
`#[cfg(feature = "test-util")]`:

```text
/// # #[cfg(feature = "test-util")]
/// # use http_extensions::{HttpResponseBuilder, FakeHandler};
```

## Multiple required features

When several features are needed, combine them with `all(...)`:

```text
/// # #[cfg(all(feature = "retry", feature = "timeout", feature = "tower-service"))] {
```

## Indented code blocks (Markdown lists)

Inside a numbered or bulleted list, the code fence and its contents are
indented relative to `///`. The injected `# #[cfg(...)]` and `# fn main`
lines must match that inner indentation, otherwise they fall outside the
code block and are parsed as prose.
