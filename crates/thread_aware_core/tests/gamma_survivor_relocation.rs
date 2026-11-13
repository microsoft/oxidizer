// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Source-aware relocation through public container implementations.

#![cfg(feature = "std")]

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use thread_aware_core::__private::v1::{new_numa_node, new_owner, new_thread};
use thread_aware_core::{Thread, ThreadAware};

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
fn gamma_survivor_containers_forward_source_and_destination_exactly_once() {
    let owner = new_owner();
    let id = std::thread::current().id();
    let source = new_thread(owner.clone(), id, new_numa_node(0));
    let destination = new_thread(owner, id, new_numa_node(1));
    let visits = Arc::new(Mutex::new(Vec::new()));
    let tracker = || Tracker(Arc::clone(&visits));
    let mut containers: Vec<Box<dyn ThreadAware>> = vec![
        Box::new([tracker(), tracker()]),
        Box::new(Some(tracker())),
        Box::new(Ok::<_, Tracker>(tracker())),
        Box::new(Err::<Tracker, _>(tracker())),
        Box::new(vec![tracker(), tracker()]),
        Box::new(VecDeque::from([tracker(), tracker()])),
        Box::new(Box::new(tracker())),
        Box::new(BTreeMap::from([(1, tracker()), (2, tracker())])),
        Box::new(HashMap::from([(1, tracker()), (2, tracker())])),
    ];
    for container in &mut containers {
        container.relocate(Some(&source), &destination);
    }
    assert_eq!(*visits.lock().unwrap(), vec![(Some(source.clone()), destination); 14]);
    visits.lock().unwrap().clear();
    for container in &mut containers {
        container.relocate(None, &source);
    }
    assert_eq!(*visits.lock().unwrap(), vec![(None, source); 14]);
}
