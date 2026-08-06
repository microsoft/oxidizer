# `#[display(...)]`

How `#[derive(ohno::Error)]` turns a template into the error's message.

## Form

```rust
#[display("template")]
#[display("template", arg1, arg2)]
```

The template is a `format!` string. Anything that is not a plain field name goes
in the argument list.

## Placeholders

| Placeholder | Meaning |
| --- | --- |
| `{name}` | the field called `name` |
| `{0}` | tuple field 0 |
| `{name:spec}` | the field, with `spec` as the format spec |
| `{}` | the next argument |
| `{{`, `}}` | a literal brace |

`{name}` and `{0}` are looked up in the struct's own fields. They expand to
`&self.<field>`. A name that is not a field is an error, reported at the
template, and the message lists the names that do work.

`{}` takes the next argument from the list. Too few or too many arguments is an
error, also reported at the template.

## Arguments are scoped to `self`

Each argument expands to `&self.<argument>`. So a field is written by its bare
name. The `self.` part is added for you. This is where the crate differs from
`thiserror`.

| Argument | Works |
| --- | --- |
| `path.display()` | yes |
| `count * 2` | yes |
| `self.path.display()` | no, `self.` is already there |
| `.path.display()` | no, not valid Rust |

`self.` lands in front of the argument's leftmost term, so that term is the one
that must name a field. The macro finds it by walking left through field access,
method calls, indexing, binary operators, casts, `await`, `?`, and ranges. So
`count * 2` is rooted at `count`, and `t.0.message()` is rooted at `t`.

If that term is not a field, the error points at the term, not at the whole
argument and not at the attribute.

The macro then checks the argument can take the prefix at all. It builds
`&self.<argument>` and tries to parse it. That is simpler than listing every
expression that may follow a dot.

## Which fields you can name

Every field the user wrote. By name for a named struct, by index for a tuple
struct.

The `OhnoCore` field added by `#[ohno::error]` is left out. Printing it would
print the error's own chain, and naming it in an error message would point at a
field that is not in the user's code. A core field the user wrote is *not* left
out. It is plain data. See `error_error.md`.

Raw identifiers keep their prefix. A field declared as `r#type` is written
`{r#type}`, and error messages offer `` `r#type` ``, because `type` alone does
not parse there.

## Error reporting

The macro reports all of the above while it expands. Without that, a bad
argument reaches `rustc` as a field access in code the user cannot see, and the
added `OhnoCore` field shows up in rustc's own list of available fields.

Errors point at what the user wrote: at the template for a template error, at
the offending term for an argument error.

An error covering more than one token is built with `syn::Error::new_spanned`,
not from a node span. A node span covers the whole node only where `Span::join`
exists, and shrinks to the first token elsewhere. The same error would then
underline different amounts of code on different toolchains, and no single
`.stderr` snapshot could match both.

## What gets rejected

| Input | Message |
| --- | --- |
| `{nope}` | unknown field `nope` in `#[display(...)]`, available fields: … |
| `{}` with no argument left | Not enough arguments for format placeholders |
| an argument no `{}` uses | Too many arguments for format placeholders |
| `self.path.display()` | positional arguments are implicitly scoped to `self` |
| `Self::LABEL.len()`, `"prefix".len()` | each argument must be rooted in a field or method of `self` |
