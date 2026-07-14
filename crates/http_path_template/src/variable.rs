// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::segment::Segment;

/// A `{field.path=sub-template}` variable binding within a [`PathTemplate`](crate::PathTemplate).
///
/// The sub-template ([`Variable::segments`]) only ever yields
/// [`Segment::Literal`], [`Segment::Single`], and [`Segment::Rest`] elements;
/// nested variables are rejected at parse time.
///
/// All strings borrow from the parsed template, so a `Variable` is a lightweight
/// [`Copy`] view that allocates nothing.
///
/// # Examples
///
/// ```
/// use http_path_template::{Grammar, PathTemplate, Segment};
///
/// # fn main() -> Result<(), http_path_template::ParseError> {
/// let template = PathTemplate::parse("/v1/{book.name=books/*}", Grammar::default())?;
/// let Segment::Variable(variable) = template.segments()[1] else {
///     panic!("expected variable")
/// };
/// assert_eq!(variable.field_path(), "book.name");
/// assert_eq!(variable.segments().count(), 2);
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Variable<'a> {
    /// The dotted field path, e.g. `shelf.id`.
    field: &'a str,
    /// The sub-template after `=`, e.g. `books/**`. The `{field}` shorthand is
    /// normalized to `*` so that `{field}` and `{field=*}` compare equal.
    sub: &'a str,
}

impl<'a> Variable<'a> {
    /// Creates a variable binding from its dotted field path and (normalized)
    /// sub-template string.
    pub(crate) fn new(field: &'a str, sub: &'a str) -> Self {
        Self { field, sub }
    }

    /// The dotted message-field path this variable binds into, e.g. `shelf.id`
    /// for `{shelf.id}`. Split on `.` for the individual identifiers.
    ///
    /// # Examples
    ///
    /// ```
    /// use http_path_template::{Grammar, PathTemplate, Segment};
    ///
    /// # fn main() -> Result<(), http_path_template::ParseError> {
    /// let template = PathTemplate::parse("/v1/{shelf.id}", Grammar::default())?;
    /// let Segment::Variable(variable) = template.segments()[1] else {
    ///     panic!("expected variable")
    /// };
    /// assert_eq!(variable.field_path(), "shelf.id");
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn field_path(&self) -> &'a str {
        self.field
    }

    /// The sub-template segments this variable captures, yielded lazily. For a
    /// shorthand `{field}` this is a single [`Segment::Single`].
    ///
    /// # Examples
    ///
    /// ```
    /// use http_path_template::{Grammar, PathTemplate, Segment};
    ///
    /// # fn main() -> Result<(), http_path_template::ParseError> {
    /// let template = PathTemplate::parse("/v1/{name=shelves/*}", Grammar::default())?;
    /// let Segment::Variable(variable) = template.segments()[1] else {
    ///     panic!("expected variable")
    /// };
    /// assert!(
    ///     variable
    ///         .segments()
    ///         .eq([Segment::Literal("shelves"), Segment::Single])
    /// );
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn segments(&self) -> SegmentIter<'a> {
        SegmentIter { rest: Some(self.sub) }
    }

    /// The raw sub-template substring (`*` for the `{field}` shorthand). Used by
    /// `Display` to render the sub-template verbatim.
    pub(crate) fn sub(&self) -> &'a str {
        self.sub
    }
}

/// A lazy iterator over a [`Variable`]'s sub-template [`Segment`]s.
///
/// Created by [`Variable::segments`]. Because the sub-template was validated at
/// parse time, iteration is infallible and allocation-free.
#[derive(Debug, Clone)]
pub struct SegmentIter<'a> {
    /// The remainder of the sub-template still to yield, or `None` once
    /// exhausted. Splitting on the ASCII `/` byte keeps `str` boundaries valid.
    rest: Option<&'a str>,
}

impl<'a> Iterator for SegmentIter<'a> {
    type Item = Segment<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let rest = self.rest?;
        let seg = if let Some(idx) = rest.as_bytes().iter().position(|&b| b == b'/') {
            // `rest[idx]` is the ASCII `/`; split there and drop it from the tail.
            let (seg, after) = rest.split_at(idx);
            self.rest = Some(&after[1..]);
            seg
        } else {
            self.rest = None;
            rest
        };
        Some(match seg {
            "*" => Segment::Single,
            "**" => Segment::Rest,
            other => Segment::Literal(other),
        })
    }
}
