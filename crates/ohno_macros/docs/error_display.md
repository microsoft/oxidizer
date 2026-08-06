# `#[display(...)]`

How `#[derive(ohno::Error)]` turns a display template into the error's message.

## Form

```rust
#[display("template")]
#[display("template", arg1, arg2)]
```

The template is a `format!` string. Anything that is not a plain field
reference is passed as a positional argument.

## Template placeholders

| Placeholder | Meaning |
| --- | --- |
| `{name}` | the field called `name` |
| `{0}` | the tuple field at index 0 |
| `{name:spec}` | the field, with `spec` as the format specifier |
| `{}` | the next positional argument |
| `{{`, `}}` | a literal brace |

A named or indexed placeholder is resolved against the struct's own fields and
expands to `&self.<field>`. A field that does not exist is reported at the
template literal, listing the fields that can be referenced.

`{}` consumes the next argument from the argument list. Too few or too many
arguments for the placeholders present is reported at the template literal.

## Positional arguments are scoped to `self`

Each argument is expanded as `&self.<argument>`, so a field is referenced by its
bare name. The `self.` prefix is implicit and must be omitted, which is where
this differs from `thiserror`.

| Argument | Accepted |
| --- | --- |
| `path.display()` | yes |
| `count * 2` | yes |
| `self.path.display()` | no, the `self.` prefix is implicit |
| `.path.display()` | no, not a valid expression |

Because the prefix lands immediately before the argument's leftmost term, that
term is the one that has to name a field. It is found by walking the expression
down through the forms that keep a term in leftmost position: field access,
method calls, indexing, binary operators, casts, `await`, `?`, and ranges. So
`count * 2` is rooted in `count`, and `t.0.message()` is rooted in `t`.

A root that names no field is reported at the term itself, not at the whole
expression or at the attribute. Whether an argument can carry the prefix at all
is then decided by parsing the expansion, rather than by enumerating the
expression forms that may legally follow a dot.

## Which fields can be referenced

Every field the user declared, by name for a named struct and by index for a
tuple struct.

The `OhnoCore` field injected by `#[ohno::error]` is excluded. Referencing it
would print the error's own chain, and naming it in a diagnostic would point at
a field absent from the user's source. A core field the user declared themselves
is *not* excluded — it is ordinary data. See `error_error.md`.

Raw identifiers keep their prefix throughout. A field declared as `r#type` is
written `{r#type}` in a template and is offered as `` `r#type` `` in a
diagnostic, because `type` would name something that does not parse there.

## Diagnostics

Everything above is reported by the macro during expansion. Without that, a
mis-scoped, misspelled or unsupported argument reaches `rustc` as a field access
in generated code the user cannot see, and the injected `OhnoCore` field leaks
into rustc's own "available fields" note.

Spans are placed at what the user wrote: at the template literal for a template
error, and at the offending term for an argument error. A diagnostic covering
more than one token is built with `syn::Error::new_spanned` rather than from a
node span, because a node span covers the whole node only where `Span::join` is
available and collapses to the first token elsewhere — which would render the
same diagnostic differently between toolchains.

## Rejected inputs

| Input | Reported as |
| --- | --- |
| `{nope}` | unknown field `nope` in `#[display(...)]`, available fields: … |
| `{}` with no argument left | Not enough arguments for format placeholders |
| an argument no `{}` consumes | Too many arguments for format placeholders |
| `self.path.display()` | positional arguments are implicitly scoped to `self` |
| `Self::LABEL.len()`, `"prefix".len()` | each argument must be rooted in a field or method of `self` |
