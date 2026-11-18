// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Relocation forwarding and async closure formatting.

use std::sync::{Arc, Mutex};

use thread_aware::closure::{async_closure, async_closure_mut, async_closure_once, closure, closure_mut, closure_once};
use thread_aware::{Thread, ThreadAware, ThreadBuilder};

type Visit = (Option<Thread>, Thread);

#[cfg(test)]
struct Tracker(Arc<Mutex<Vec<Visit>>>);

#[cfg(test)]
impl ThreadAware for Tracker {
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        self.0.lock().unwrap().push((source.cloned(), destination.clone()));
    }
}

#[test]
fn gamma_survivor_all_closure_kinds_forward_original_source() {
    let builder = ThreadBuilder::default();
    let id = std::thread::current().id();
    let source = builder.clone().with_numa_node(7).build(id);
    let destination = builder.with_numa_node(11).build(id);
    let visits = Arc::new(Mutex::new(Vec::new()));
    let tracker = || Tracker(Arc::clone(&visits));
    let mut closures: Vec<Box<dyn ThreadAware>> = vec![
        Box::new(closure(tracker(), |_| ())),
        Box::new(closure_once(tracker(), |_| ())),
        Box::new(closure_mut(tracker(), |_| ())),
        Box::new(async_closure(tracker(), |_| Box::pin(async {}))),
        Box::new(async_closure_once(tracker(), |_| Box::pin(async {}))),
        Box::new(async_closure_mut(tracker(), |_| Box::pin(async {}))),
    ];
    for value in &mut closures {
        value.relocate(Some(&source), &destination);
    }
    assert_eq!(*visits.lock().unwrap(), vec![(Some(source), destination); 6]);
}

#[test]
fn gamma_survivor_thread_builder_preserves_exact_numa_and_default_zero() {
    let id = std::thread::current().id();
    assert_eq!(format!("{:?}", ThreadBuilder::default().build(id).numa_node()), "NumaNode(0)");
    for node in [0, 7, u32::MAX] {
        assert_eq!(
            format!("{:?}", ThreadBuilder::default().with_numa_node(node).build(id).numa_node()),
            format!("NumaNode({node})")
        );
    }
}

#[test]
fn gamma_survivor_async_closures_name_their_captured_data() {
    let shared = async_closure(13, |x| Box::pin(async move { *x }));
    let once = async_closure_once(17, |x| Box::pin(async move { x }));
    let mutable = async_closure_mut(19, |x| Box::pin(async move { *x }));
    assert_eq!(format!("{shared:?}"), "AsyncClosure { data: 13, .. }");
    assert_eq!(format!("{once:?}"), "AsyncClosureOnce { data: 17, .. }");
    assert_eq!(format!("{mutable:?}"), "AsyncClosureMut { data: 19, .. }");
}

#[cfg(feature = "test-utils")]
#[test]
fn gamma_survivor_relocator_uses_node_one_and_reenables_source() {
    let relocator = thread_aware::Relocator::between_numa_nodes().source(false).source(true);
    let (source, destination) = relocator.relocate(&mut ());
    assert_eq!(format!("{:?}", source.unwrap().numa_node()), "NumaNode(0)");
    assert_eq!(format!("{:?}", destination.numa_node()), "NumaNode(1)");
}
