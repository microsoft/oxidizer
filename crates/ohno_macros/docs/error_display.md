# `#[display(...)]`

The message a generated error renders.

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

## Arguments are scoped to `self`

A field is written by its bare name. The `self.` is implicit, which is where the
crate differs from `thiserror`.

| Argument | Works |
| --- | --- |
| `path.display()` | yes |
| `count * 2` | yes |
| `self.path.display()` | no, `self.` is already there |
| `.path.display()` | no, not valid Rust |

An argument is scoped by its leftmost term, so that term is the one that has to
name a field: `count * 2` is rooted at `count`, and `t.0.message()` at `t`. An
argument rooted anywhere else — a constant, an associated function, a literal —
names no field of `self` and is rejected.

The operator applies to the field's value, not to a reference to it, so
`count as u64` casts the field and `count * 2` uses the value's `Mul`.

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

The macro reports these itself rather than letting the expansion fail. Otherwise
a bad argument reaches `rustc` as a field access in code the user cannot see,
and the added `OhnoCore` field appears in rustc's own list of available fields.

Errors point at what the user wrote: at the template for a template error, at
the offending term for an argument error.

An error covering more than one token has to be built with
`syn::Error::new_spanned`, not from a node span. A node span covers the whole
node only where `Span::join` exists, and shrinks to the first token elsewhere.
The same error would then underline different amounts of code on different
toolchains, and no single `.stderr` snapshot could match both.

## What gets rejected

- A placeholder naming something that is not a field.
- A `{}` with no argument left, or an argument no `{}` uses.
- An argument written with a `self.` prefix.
- An argument rooted in anything other than a field or method of `self`.
