// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// A matched route's name and captured path variables.
pub trait RouteMatch<'p> {
    /// The matched route's registered name.
    fn name(&self) -> &str;

    /// The captured value of the path variable `name`, or [`None`] if the matched
    /// route has no such variable.
    ///
    /// `name` is the template variable exactly as written, including dotted
    /// names and Rust keywords.
    fn capture(&self, name: &str) -> Option<&'p str>;
}
