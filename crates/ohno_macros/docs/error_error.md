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
   user wrote. The marker is internal; its text is in
   `GENERATED_ERROR_FIELD_MARKER`.
2. **A field with `#[error]`.** This is how a user picks the field by hand.
3. **A field whose type is named `OhnoCore`.** This runs only when no field is
   marked. There must be exactly one such field.

Step 3 reads the last part of the type path and resolves nothing, so it does not
find a type alias or a renamed import.

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
error from the field it adds. Three things follow.

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

**`#[no_constructors]` is not accepted.** Opting out of the generated
constructors means writing the struct literal by hand, and that needs the name
of the field this attribute adds. The name is not the author's to rely on: it is
`ohno_core`, unless the struct already declares one, in which case the added
field becomes `ohno_core_1`. A constructor would then name a field chosen for it,
which a later, unrelated field declaration can rename out from under it.

Declaring the core is what makes a hand-written constructor honest, so that is
the supported path:

```rust
#[derive(ohno::Error)]
#[no_constructors]
struct MyError {
    path: String,
    ohno_core: ohno::OhnoCore,
}
```

## Why the added field uses a doc marker

A derive helper attribute must be listed in `attributes(...)` to be inert, and
everything in that list shows up in the derive's public rustdoc. A doc string
needs no such listing. The added field is private, and rustdoc does not print
private fields, so the marker stays out of the docs.

The cost falls on the derive. It runs after `#[ohno::error]` has added the
field, so it cannot tell a marker it was handed from one a user wrote, and can
only compare text. That is why the marker ends in a nonce: an ordinary doc
comment will not match it.

`#[ohno::error]` is under no such limit. It runs before it adds anything, so a
marker present at that point was written by hand, and it says so. One would take
over the error field, and two would settle the choice by declaration order.

If a struct carries both markers, the macro reports that too. It matters because
field lookup takes the first marked field it sees.

## What gets rejected

- `#[error]` with any argument.
- Two marked fields, or one field marked twice.
- `#[error]` in a struct that already has the added field.
- A hand-written reserved doc marker under `#[ohno::error]`.
- `#[error]` under `#[ohno::error]`.
- `#[no_constructors]` under `#[ohno::error]`.
- No marker and no `OhnoCore` field, or no marker and several of them.
- An enum, or a unit struct under the derive alone.

## Implementation notes

**The struct is parsed, validated and selected from in one call.** Selection
takes the first picked field, which is only unambiguous once validation has
ruled out the ways it can. Keeping them one operation makes that order
structural rather than a rule the caller has to remember.

**Rejection happens where the input is still the user's.** `#[ohno::error]` runs
before it adds anything, so it can say a marker was hand-written. The derive
runs after, cannot tell the two apart, and so only compares text.

**A marker beside the added field cannot settle the choice by declaration
order.** That combination is reported, so field lookup taking the first match
never resolves it silently.

Under the derive alone this does not hold: validation counts `#[error]`
attributes, not generated ones, so two hand-written reserved markers pass it and
field lookup takes the first. That is the text-comparison limit above, and
`#[ohno::error]` closes the path a user realistically takes.

## Limits

Generated code refers to the crate as `ohno`. Renaming the package in
`Cargo.toml` is not supported.
