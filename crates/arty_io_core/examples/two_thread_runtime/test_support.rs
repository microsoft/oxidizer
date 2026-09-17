// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::TypeId;
use std::collections::VecDeque;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};
use std::task::Waker;
use std::thread;

use arty_io_core::{CompletionWaiter, DriverContext, DriverProvider, IoContext, LocalDriver, ProviderContext, SystemTask, SystemTasks};
use thread_aware_core::{Thread, ThreadAware};

use super::coordinator::{Coordinator, Source};
use super::native::{NativeWaiter, ReadinessClient, RecordClient};

#[derive(Default)]
pub(super) struct ManualTasks {
    queue: Arc<Mutex<VecDeque<SystemTask>>>,
}

impl ManualTasks {
    pub(super) fn handle(&self) -> SystemTasks {
        let queue = Arc::clone(&self.queue);
        SystemTasks::new(move |task| queue.lock().unwrap().push_back(task))
    }

    pub(super) fn len(&self) -> usize {
        self.queue.lock().unwrap().len()
    }

    pub(super) fn run_all(&self) {
        loop {
            let task = self.queue.lock().unwrap().pop_front();
            let Some(task) = task else {
                break;
            };
            task();
        }
    }
}

type Created<C> = (LocalDriver<<<C as IoContext>::Provider as DriverProvider>::Driver>, Arc<Source>);

pub(super) struct Harness {
    pub(super) coordinator: Coordinator<NativeWaiter>,
    pub(super) tasks: ManualTasks,
    pub(super) thread: Thread,
}

impl Harness {
    pub(super) fn new(quantum: usize) -> Self {
        Self {
            coordinator: Coordinator::new(NativeWaiter::new(), NonZeroUsize::new(quantum).unwrap()),
            tasks: ManualTasks::default(),
            thread: thread_aware_core::__private::v1::new_thread(
                thread_aware_core::__private::v1::new_owner(),
                thread::current().id(),
                thread_aware_core::__private::v1::new_numa_node(0),
            ),
        }
    }

    pub(super) fn create<C: IoContext>(&self) -> Created<C> {
        let source = Source::new(self.coordinator.waiter.waker());
        let domain = self.coordinator.waiter.domain();
        let context = DriverContext::new(
            self.thread.clone(),
            self.tasks.handle(),
            domain.clone(),
            Waker::from(Arc::clone(&source)),
        )
        .with_completion_service(domain.service(self.coordinator.waiter.record_client()))
        .unwrap()
        .with_completion_service(domain.service(self.coordinator.waiter.readiness_client()))
        .unwrap();
        let mut provider = C::provider(
            ProviderContext::new()
                .with_completion_service::<RecordClient>()
                .with_completion_service::<ReadinessClient>(),
        )
        .unwrap();
        provider.relocate(None, &self.thread);
        provider.completion_requirements().validate(&context).unwrap();
        (provider.create(context).unwrap(), source)
    }

    pub(super) fn install<C: IoContext>(&mut self) -> C {
        let (driver, source) = self.create::<C>();
        let context = driver.context();
        self.coordinator.insert(TypeId::of::<C>(), source, Box::new(driver));
        context
    }
}
