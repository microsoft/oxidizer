// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Context retrieval during provider callbacks.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::task::Waker;
use std::time::Duration;

use arty_io_core::{Cycle, Driver, DriverError, DriverOptions, IoContext};
use arty_io_host::{DriverHost, IoRuntime};
use observed::Sink;
use thread_aware::{Thread, ThreadAware, ThreadBuilder};

thread_local! {
    static HOST: RefCell<Option<Rc<DriverHost>>> = const { RefCell::new(None) };
}

#[derive(Clone)]
struct TestContext {
    during_initialization: bool,
    errors: Arc<Mutex<Vec<String>>>,
}

impl ThreadAware for TestContext {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

impl TestContext {
    fn retrieve_again(&self) {
        let host = HOST.with_borrow(|host| Rc::clone(host.as_ref().unwrap()));
        let error = host.context::<Self>(self.clone()).err().unwrap();
        self.errors.lock().unwrap().push(error.to_string());
    }
}

impl IoContext for TestContext {
    type ContextOptions = Self;
    type Driver = Self;

    fn new(options: Self::ContextOptions) -> Self {
        options
    }

    fn driver(&mut self, _options: DriverOptions) -> Result<Self::Driver, DriverError> {
        if self.during_initialization {
            self.retrieve_again();
        }
        Ok(self.clone())
    }
}

impl Driver for TestContext {
    fn waker(&self) -> Waker {
        Waker::noop().clone()
    }

    fn execute_cycle(&mut self, _cycle: &mut Cycle) -> Result<(), DriverError> {
        self.retrieve_again();
        Ok(())
    }

    fn shutdown(self) -> Result<(), DriverError> {
        Ok(())
    }
}

fn exercise_retrieval(during_initialization: bool) -> Vec<String> {
    let thread = ThreadBuilder::default().build(std::thread::current().id());
    let runtime = IoRuntime::new(Sink::noop());
    let host = Rc::new(runtime.driver_host(thread, Waker::noop().clone()).unwrap());
    HOST.with_borrow_mut(|installed| *installed = Some(Rc::clone(&host)));
    let errors = Arc::new(Mutex::new(Vec::new()));
    let context = TestContext {
        during_initialization,
        errors: Arc::clone(&errors),
    };

    host.context::<TestContext>(context.clone()).unwrap();
    if !during_initialization {
        host.execute_cycle(Duration::ZERO).unwrap();
    }
    host.context::<TestContext>(context).unwrap();
    HOST.with_borrow_mut(Option::take);
    errors.lock().unwrap().clone()
}

#[test]
fn nested_context_retrieval_is_rejected_during_initialization() {
    assert_eq!(
        exercise_retrieval(true),
        ["I/O context retrieval cannot be nested during initialization"]
    );
}

#[test]
fn context_retrieval_is_rejected_while_drivers_are_borrowed() {
    assert_eq!(
        exercise_retrieval(false),
        ["I/O context retrieval cannot overlap driver cycle execution"]
    );
}
