# `#[error]` and the `OhnoCore` field

How `#[derive(ohno::Error)]` picks the field that holds the `OhnoCore`, and how
`#[ohno::error]` fits in.

## The error field

Each generated error has one *error field*. It holds the `OhnoCore`.

The macro reads that field for `source()`, for the backtrace, for enrichment,
and for the `caused by:` part of `Display`. All other fields are plain data.

## Picking the error field

The macro tries three things, in this order.

1. **A field with the reserved `#[doc]` marker.** `#[ohno::error]` puts that doc
   string on the field it adds, so the macro can tell it apart from a core the
   user wrote. The marker is internal. Its text is in
   `GENERATED_ERROR_FIELD_MARKER`.
2. **A field with `#[error]`.** This is how a user picks the field by hand.
3. **A field whose type is named `OhnoCore`.** This runs only when no field is
   marked. There must be exactly one such field.

Step 3 looks at the last part of the type path. So it finds `OhnoCore`,
`ohno::OhnoCore` and `crate::OhnoCore`. It does not resolve types, so it does
not find a type alias or a renamed import.

That is why the macro does not check the type of a marked field. A user who
marks a field has said which field it is. The type is left to `rustc`, which can
resolve the alias. Without this, an aliased core could not be used at all.

```rust
type Core = ohno::OhnoCore;

// Rejected. Step 3 looks for a type named `OhnoCore`, and this one is spelled
// `Core`, so no error field is found.
#[derive(ohno::Error)]
#[display("failed for {path}")]
struct AliasedError {
    path: String,
    inner: Core,
}

// Accepted. The marker names the field, so how its type is spelled stops
// mattering.
#[derive(ohno::Error)]
#[display("failed for {path}")]
struct AliasedError {
    path: String,
    #[error]
    inner: Core,
}
```

## Rules for `#[error]`

`#[error]` is a bare word. It takes no arguments. `#[error(anything)]`,
`#[error()]` and `#[error = "x"]` are all errors, not warnings.

Only one field may be marked. One field may not carry the marker twice.

Marking is optional. Most errors have one `OhnoCore` field and no marker, and
step 3 finds it.

## `#[ohno::error]`

This attribute always adds the `OhnoCore` field itself. It always builds the
error from the field it adds. Two things follow.

**No field may be marked with `#[error]`.** A marker asks for a different field,
and the attribute cannot do that. To place the core by hand, use
`#[derive(ohno::Error)]` on its own.

**A user's own `OhnoCore` field is plain data.** It goes into the generated
constructors and into `Debug`, and a `#[display(...)]` template can print it.
The macro never reads it for `source()`, the backtrace, or enrichment.

```rust
#[ohno::error]
#[display("failed for {path}, carrying {carried}")]
struct DeclaredCoreError {
    path: String,
    carried: ohno::OhnoCore,   // data, not the error
}
```

On a unit struct, the attribute rewrites the struct as a tuple struct holding
the added field. `#[derive(ohno::Error)]` alone rejects a unit struct, because
there is no room for a core.

## Why the added field uses a doc marker

A derive helper attribute must be listed in `attributes(...)` to be inert, and
everything in that list shows up in the derive's public rustdoc. A doc string
needs no such listing. The added field is private, and rustdoc does not print
private fields, so the marker stays out of the docs.

The cost is that a doc string cannot be rejected. A user may write one, and the
macro can only compare text. So the marker text ends in a nonce, which an
ordinary doc comment will not match.

If a struct carries both markers, the macro reports it. That keeps the two
apart, which matters because field lookup takes the first marked field it sees.

## What gets rejected

| Input | Message |
| --- | --- |
| `#[error(...)]`, `#[error()]`, `#[error = "x"]` | `` `#[error]` takes no arguments `` |
| Two marked fields | Multiple fields marked with `` `#[error]` `` |
| One field marked twice | Duplicate `` `#[error]` `` on the same field |
| `#[error]` next to the added field | `#[ohno::error]` already added the field holding the OhnoCore |
| `#[error]` under `#[ohno::error]` | `#[ohno::error]` adds the OhnoCore field itself |
| No marker, no `OhnoCore` field | No field marked with `` `#[error]` `` found |
| No marker, several `OhnoCore` fields | Multiple OhnoCore fields found |
| An enum, or a unit struct under the derive alone | Not supported |

## Limits

Generated code refers to the crate as `ohno`. Renaming the package in
`Cargo.toml` is not supported.
