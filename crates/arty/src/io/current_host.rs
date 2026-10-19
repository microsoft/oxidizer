// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::task::Waker;
use std::time::Duration;

use arty_io_core::IoContext;
use arty_io_host::{DriverHost, IoRuntime};
use thread_aware::Thread;

use crate::io::fail_runtime;
use crate::runtime::Error;
use crate::runtime::dispatch::DispatcherClient;

thread_local! {
    static CURRENT: RefCell<Option<Weak<RuntimeDriverHost>>> = const { RefCell::new(None) };
}

#[derive(Debug)]
pub(crate) struct RuntimeDriverHost {
    runtime: IoRuntime,
    dispatcher: DispatcherClient,
    thread: Thread,
    host: DriverHost,
}

impl RuntimeDriverHost {
    pub(crate) fn new(runtime: &IoRuntime, dispatcher: DispatcherClient, thread: &Thread, owner_waker: Waker) -> Rc<Self> {
        let driver_host = runtime.clone().driver_host(owner_waker);
        let host = Rc::new(Self {
            runtime: runtime.to_owned(),
            dispatcher,
            thread: thread.to_owned(),
            host: driver_host,
        });
        CURRENT.with_borrow_mut(|current| {
            assert!(
                current.as_ref().and_then(Weak::upgrade).is_none(),
                "an I/O driver host is already installed on this worker"
            );
            *current = Some(Rc::downgrade(&host));
        });
        host
    }

    pub(crate) fn waker(&self) -> Waker {
        self.host.waker()
    }

    pub(crate) fn try_context_current<C>(runtime: &IoRuntime, thread: &Thread, options: &C::ContextOptions) -> Option<Result<C, Error>>
    where
        C: IoContext,
        C::ContextOptions: Clone,
    {
        Self::current(runtime, thread).map(|host| {
            let context = host
                .host
                .context::<C>(options)
                .map_err(|error| fail_runtime(&host.dispatcher, error))?;
            crate::io::runtime_error(&host.dispatcher).map_or(Ok(context), Err)
        })
    }

    fn current(runtime: &IoRuntime, thread: &Thread) -> Option<Rc<Self>> {
        CURRENT.with_borrow(|current| {
            current
                .as_ref()
                .and_then(Weak::upgrade)
                .filter(|host| host.runtime == *runtime && host.thread == *thread)
        })
    }

    pub(crate) fn execute_cycle(&self, max_wait: Duration) -> Result<(), Error> {
        self.host
            .execute_cycle(max_wait)
            .map_err(|error| fail_runtime(&self.dispatcher, error))
    }
}
