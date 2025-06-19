// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Instructs the runtime to schedule a specific number of instances of a task, distributed across
/// the [selected worker thread placement category](super::Placement).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Instantiation {
    /// A task will be scheduled on every worker thread in the selected category.
    All,
    // TODO: Implement additional instantiation modes that we require in our vetted use cases,
    // such as <???>.
}