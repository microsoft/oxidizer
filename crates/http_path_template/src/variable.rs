// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::segment::Segment;

/// A `{field.path=sub-template}` variable binding within a [`PathTemplate`](crate::PathTemplate).
///
/// The sub-template ([`Variable::segments`]) only ever contains
/// [`Segment::Literal`], [`Segment::Single`], and [`Segment::Rest`] elements;
/// nested variables are rejected at parse time.
///
/// # Examples
///
/// ```
/// use http_path_template::{Grammar, PathTemplate, Segment};
///
/// # fn main() -> Result<(), http_path_template::ParseError> {
/// let template = PathTemplate::parse("/v1/{book.name=books/*}", Grammar::default())?;
/// let Segment::Variable(variable) = &template.segments()[1] else {
///     panic!("expected variable")
/// };
/// assert_eq!(
///     variable.field_path(),
///     &[String::from("book"), String::from("name")]
/// );
/// assert_eq!(variable.segments().len(), 2);
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Variable {
    field_path: Vec<String>,
    segments: Vec<Segment>,
}

impl Variable {
    /// Creates a variable binding from its field path and sub-template segments.
    pub(crate) fn new(field_path: Vec<String>, segments: Vec<Segment>) -> Self {
        Self { field_path, segments }
    }

    /// The dotted message-field path this variable binds into, e.g.
    /// `["shelf", "id"]` for `{shelf.id}`.
    ///
    /// # Examples
    ///
    /// ```
    /// use http_path_template::{Grammar, PathTemplate, Segment};
    ///
    /// # fn main() -> Result<(), http_path_template::ParseError> {
    /// let template = PathTemplate::parse("/v1/{shelf.id}", Grammar::default())?;
    /// let Segment::Variable(variable) = &template.segments()[1] else {
    ///     panic!("expected variable")
    /// };
    /// assert_eq!(
    ///     variable.field_path(),
    ///     &[String::from("shelf"), String::from("id")]
    /// );
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn field_path(&self) -> &[String] {
        &self.field_path
    }

    /// The sub-template segments this variable captures. For a shorthand
    /// `{field}` this is a single [`Segment::Single`].
    ///
    /// # Examples
    ///
    /// ```
    /// use http_path_template::{Grammar, PathTemplate, Segment};
    ///
    /// # fn main() -> Result<(), http_path_template::ParseError> {
    /// let template = PathTemplate::parse("/v1/{name=shelves/*}", Grammar::default())?;
    /// let Segment::Variable(variable) = &template.segments()[1] else {
    ///     panic!("expected variable")
    /// };
    /// assert_eq!(
    ///     variable.segments(),
    ///     &[Segment::Literal(String::from("shelves")), Segment::Single]
    /// );
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }
}
