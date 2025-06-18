// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use isolated_domains::Domain;

/// Instructs the runtime to limit candidate worker threads to a specific set when selecting which
/// thread to execute a task on.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Placement {
    /// All worker threads are valid candidates for receiving the task. This is the default.
    #[default]
    Any,

    /// The task may be placed on any worker thread in the current memory region.
    ///
    /// The term "current" refers to the memory region associated with the thread that is spawning
    /// the task.
    CurrentRegion,

    /// The task may be placed on any async background worker thread.
    ///
    #[doc = include_str!("../../doc/snippets/background_task.md")]
    Background,

    /// Use `PlacementToken` obtained from another task to place the task on the same runtime thread.
    SameThreadAs(PlacementToken),
    // TODO: Implement additional instantiation modes that we require in our vetted use cases,
    // such as Auto, CurrentRegion, Region(region_id), SameAs(task_coordinates),  EfficiencyProcessorsOnly or ...
}

// TODO: Remove PlacementToken in favor of using Domain directly.

/// A token that represents placement of a task on runtime controlled threads
///
/// ```
/// # use oxidizer_rt::{Placement, PlacementToken, Runtime, BasicThreadState};
/// let rt = Runtime::<BasicThreadState>::new().expect("Failed to create runtime");
/// let mut join_handle = rt.spawn(async |_cx| {std::thread::current().id()});
///
/// // Get the placement token from task's join handle
/// let placement_1: PlacementToken = join_handle.placement().unwrap();
///
/// // Get thread ID of the task
/// let thread_id_1 = join_handle.wait();
///
/// // Spawn a new task on the same thread as the previous task and get its own placement token from context and thread ID
/// let (placement_2, thread_id_2) = rt.spawn_with_meta(Placement::SameThreadAs(placement_1), async move |cx| {(cx.builtins().core.runtime_ops.placement().unwrap(), std::thread::current().id())}).wait();
///
/// // Spawn a new task using the second placement token and get its thread ID
/// let thread_id_3 = rt.spawn_with_meta(Placement::SameThreadAs(placement_2), async move |_cx|  {std::thread::current().id()}).wait();
///
/// // All three tasks should have the same thread ID
/// assert_eq!(thread_id_1, thread_id_2);
/// assert_eq!(thread_id_2, thread_id_3);
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlacementToken {
    /// The domain of the task that is providing the placement token
    pub(crate) domain: Domain,
}

impl PlacementToken {
    pub(crate) const fn new(domain: Domain) -> Self {
        Self { domain }
    }
}