# AI Agents Guidelines for `http_extensions`

Code in this crate should follow the [Microsoft Rust Guidelines](https://microsoft.github.io/rust-guidelines/agents/all.txt).

## Key Invariants

- `HttpBody` must only be constructed through `HttpBodyBuilder` — it manages memory pool lifecycle.
- Extension traits (`StatusExt`, `HeaderMapExt`, etc.) use the sealed trait pattern — public but not implementable outside the crate.
- `HttpError` size is asserted at 64 bytes in tests; keep it small to avoid stack bloat.
- `FakeHandler` and `new_fake()` constructors are gated behind the `test-util` feature.
- The `json` feature gates `Json<T>`, `JsonError`, and the `.json()` builder methods.

## User-Facing Best Practices

For guidance on how users should consume this crate (type selection, URI patterns,
header handling, error handling, testing), see the
[http-expert skill](skills/http-expert/prompt.md) and
[recipes](src/_documentation/recipes.rs).
