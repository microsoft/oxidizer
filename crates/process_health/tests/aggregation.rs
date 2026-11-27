// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Contract tests for publisher, component, and aggregator behavior.

use std::collections::HashSet;
use std::pin::pin;
use std::sync::{Arc, Mutex, mpsc};
use std::task::{Context, Wake, Waker};
use std::thread;
use std::time::Duration;

use future_assert::FutureTestExt;
use futures::executor::block_on;
use health::{Aggregator, Attribute, Component, Health, Monitor, Publisher, Report, Signal};
use observed::Sink;
use static_assertions::assert_impl_all;
use thread_aware::{ThreadAware, ThreadBuilder};

const RECEIVE_TIMEOUT: Duration = Duration::from_secs(if cfg!(miri) { 120 } else { 5 });

assert_impl_all!(Aggregator: Send, Sync, Clone);
assert_impl_all!(Monitor: Send, Sync, Clone);
assert_impl_all!(Component: Send, Sync, Clone);
assert_impl_all!(Publisher: Send, Sync, Clone);
assert_impl_all!(Signal: Send, Sync, Clone);
assert_impl_all!(Report: Send, Sync, Clone);
assert_impl_all!(Attribute: Send, Sync, Clone);
assert_impl_all!(Health: Send, Sync, Clone, Copy);

struct BlockingWake {
    entered: mpsc::Sender<()>,
    release: Mutex<mpsc::Receiver<()>>,
}

impl Wake for BlockingWake {
    fn wake(self: Arc<Self>) {
        self.entered.send(()).expect("test is waiting for the wake");
        let released = {
            let release = self
                .release
                .lock()
                .expect("receive failures do not unwind while the release lock is held");
            release.recv()
        };
        released.expect("test releases the waker");
    }
}

fn receive<T>(receiver: &mpsc::Receiver<T>) -> Result<T, mpsc::RecvTimeoutError> {
    receiver.recv_timeout(RECEIVE_TIMEOUT)
}

fn wait_for_component(component: &mut Component, state: Health) {
    block_on(async {
        while component.state() != state {
            component.changed().await;
        }
    });
}

fn wait_for_monitor(monitor: &mut Monitor, state: Health) {
    block_on(async {
        while monitor.state() != state {
            monitor.changed().await;
        }
    });
}

#[test]
fn empty_aggregator_and_component_are_unhealthy() {
    let aggregator = Aggregator::new(Sink::noop());
    let monitor = aggregator.monitor();
    assert_eq!(monitor.state(), Health::Unhealthy);

    let component = aggregator.component("database");
    assert_eq!(component.state(), Health::Unhealthy);
    assert_eq!(monitor.state(), Health::Unhealthy);
}

#[test]
fn attributes_accept_owned_text_and_expose_borrowed_values() {
    let attribute = Attribute::from((String::from("name"), String::from("value")));

    assert_eq!(attribute.name(), "name");
    assert_eq!(attribute.value(), "value");
}

#[test]
fn attribute_values_display_their_payload() {
    let attribute = Attribute::new("kind", "text");

    assert_eq!(attribute.to_string(), "kind=text");
}

#[test]
fn worst_publisher_and_component_win() {
    let aggregator = Aggregator::new(Sink::noop());
    let mut monitor = aggregator.monitor();
    let database = aggregator.component("database");
    let cache = aggregator.component("cache");
    let mut primary = database.publisher();
    let mut replica = database.publisher();
    let _cache = cache.publisher();
    wait_for_monitor(&mut monitor, Health::Healthy);

    replica.degraded("lagging");
    wait_for_monitor(&mut monitor, Health::Degraded);
    assert_eq!(database.state(), Health::Degraded);
    assert_eq!(monitor.state(), Health::Degraded);

    primary.publish(Health::Unhealthy, Vec::<Attribute>::new());
    wait_for_monitor(&mut monitor, Health::Unhealthy);
    assert_eq!(database.state(), Health::Unhealthy);
    assert_eq!(cache.state(), Health::Healthy);
    assert_eq!(monitor.state(), Health::Unhealthy);

    primary.publish(Health::Healthy, Vec::<Attribute>::new());
    replica.publish(Health::Healthy, Vec::<Attribute>::new());
    wait_for_monitor(&mut monitor, Health::Healthy);
}

#[test]
fn dropping_publishers_and_components_converges() {
    let aggregator = Aggregator::new(Sink::noop());
    let mut monitor = aggregator.monitor();
    let healthy = aggregator.component("healthy");
    let _healthy_publisher = healthy.publisher();
    let mut failing = aggregator.component("failing");
    let mut failing_publisher = failing.publisher();
    failing_publisher.degraded("broken");
    wait_for_monitor(&mut monitor, Health::Degraded);

    drop(failing_publisher);
    wait_for_component(&mut failing, Health::Unhealthy);
    wait_for_monitor(&mut monitor, Health::Unhealthy);

    drop(failing);
    wait_for_monitor(&mut monitor, Health::Healthy);
    assert_eq!(block_on(monitor.reports()).count(), 1);
}

#[test]
fn dropping_a_publisher_does_not_wait_for_a_notification_consumer() {
    let aggregator = Aggregator::new(Sink::noop());
    let mut component = aggregator.component("database");
    let publisher = component.publisher();
    block_on(component.changed());

    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (returned_tx, returned_rx) = mpsc::channel();
    let waker = Waker::from(Arc::new(BlockingWake {
        entered: entered_tx,
        release: Mutex::new(release_rx),
    }));
    let mut changed = pin!(component.changed());
    assert!(changed.as_mut().poll(&mut Context::from_waker(&waker)).is_pending());

    let worker = thread::spawn(move || {
        drop(publisher);
        returned_tx.send(()).unwrap();
    });

    let returned = receive(&returned_rx).is_ok();
    let notified = receive(&entered_rx).is_ok();
    let _ = release_tx.send(());
    worker.join().unwrap();
    assert!(notified, "the component change must notify its consumer");
    assert!(returned, "publisher cleanup must not wait for the consumer");
}

#[test]
fn publishing_does_not_wait_for_a_notification_consumer() {
    let aggregator = Aggregator::new(Sink::noop());
    let mut component = aggregator.component("database");
    let mut publisher = component.publisher();
    block_on(component.changed());

    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (returned_tx, returned_rx) = mpsc::channel();
    let waker = Waker::from(Arc::new(BlockingWake {
        entered: entered_tx,
        release: Mutex::new(release_rx),
    }));
    let mut changed = pin!(component.changed());
    assert!(changed.as_mut().poll(&mut Context::from_waker(&waker)).is_pending());

    let worker = thread::spawn(move || {
        publisher.degraded("slow");
        returned_tx.send(()).unwrap();
    });

    let returned = receive(&returned_rx).is_ok();
    let notified = receive(&entered_rx).is_ok();
    let _ = release_tx.send(());
    worker.join().unwrap();
    assert!(notified, "the component change must notify its consumer");
    assert!(returned, "publishing must not wait for the consumer");
}

#[test]
fn registering_a_component_does_not_wait_for_a_monitor() {
    let aggregator = Aggregator::new(Sink::noop());
    let mut monitor = aggregator.monitor();
    let (entered, waiting) = mpsc::channel();
    let (release, resume) = mpsc::channel();
    let waker = Waker::from(Arc::new(BlockingWake {
        entered,
        release: Mutex::new(resume),
    }));
    let mut changed = pin!(monitor.changed());
    assert!(changed.as_mut().poll(&mut Context::from_waker(&waker)).is_pending());

    let (registered, result) = mpsc::channel();
    thread::scope(|scope| {
        scope.spawn(|| registered.send(aggregator.component("database")).unwrap());
        let entered = receive(&waiting);
        let component = receive(&result);
        release.send(()).unwrap();
        entered.unwrap();
        assert!(component.is_ok(), "registration must not wait for the monitor's waker");
    });
}

#[test]
fn publisher_keeps_registration_during_an_in_flight_publish() {
    let aggregator = Aggregator::new(Sink::noop());
    let mut component = aggregator.component("database");
    let mut publisher = component.publisher();
    block_on(component.changed());
    let mut monitor = aggregator.monitor();
    assert_eq!(monitor.state(), Health::Healthy);

    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let waker = Waker::from(Arc::new(BlockingWake {
        entered: entered_tx,
        release: Mutex::new(release_rx),
    }));
    let mut waiting_monitor = monitor.clone();
    let mut changed = pin!(waiting_monitor.changed());
    assert!(changed.as_mut().poll(&mut Context::from_waker(&waker)).is_pending());

    let worker = thread::spawn(move || {
        publisher.degraded("slow");
        publisher
    });
    let notified = receive(&entered_rx).is_ok();
    drop(component);
    let remaining = block_on(monitor.reports()).count();
    let _ = release_tx.send(());
    let publisher = worker.join().unwrap();

    assert!(notified, "the aggregate change must notify its consumer");
    assert_eq!(remaining, 1);
    wait_for_monitor(&mut monitor, Health::Degraded);
    drop(publisher);
    wait_for_monitor(&mut monitor, Health::Unhealthy);
    assert_eq!(block_on(monitor.reports()).count(), 0);
}

#[test]
fn chained_publisher_keeps_component_registered() {
    let aggregator = Aggregator::new(Sink::noop());
    let mut monitor = aggregator.monitor();
    let mut publisher = aggregator.component("database").publisher();

    publisher.degraded("slow");
    wait_for_monitor(&mut monitor, Health::Degraded);
    assert_eq!(block_on(monitor.reports()).count(), 1);

    drop(publisher);
    wait_for_monitor(&mut monitor, Health::Unhealthy);
    assert_eq!(block_on(monitor.reports()).count(), 0);
}

#[test]
fn component_is_idempotent_by_name() {
    let aggregator = Aggregator::new(Sink::noop());
    let monitor = aggregator.monitor();
    let mut first = aggregator.component("database");
    let mut publisher = aggregator.component(String::from("database")).publisher();
    publisher.degraded("slow");
    wait_for_component(&mut first, Health::Degraded);
    assert_eq!(block_on(monitor.reports()).count(), 1);
}

#[test]
#[should_panic(expected = "empty")]
fn empty_component_name_panics() {
    let _component = Aggregator::new(Sink::noop()).component("");
}

#[test]
fn cloned_publisher_is_independent_and_healthy() {
    let aggregator = Aggregator::new(Sink::noop());
    let mut component = aggregator.component("database");
    let mut original = component.publisher();
    original.degraded("slow");

    let clone = original.clone();
    wait_for_component(&mut component, Health::Degraded);

    drop(original);
    wait_for_component(&mut component, Health::Healthy);
    drop(clone);
    wait_for_component(&mut component, Health::Unhealthy);
}

#[test]
fn publisher_with_starts_with_the_requested_signal() {
    let aggregator = Aggregator::new(Sink::noop());
    let mut component = aggregator.component("database");
    let _publisher = component.publisher_with(Health::Unhealthy, [("reason", "starting")]);

    let report = block_on(async {
        loop {
            let report = component.report().await;
            if report.publisher_count(Health::Unhealthy) == 1 {
                break report;
            }
            component.changed().await;
        }
    });
    assert_eq!(report.publisher_count(Health::Healthy), 0);
    assert_eq!(report.publisher_count(Health::Unhealthy), 1);
    let (signal, count) = report.signals(Health::Unhealthy).next().unwrap();
    assert_eq!(count, 1);
    let mut attributes = signal.attributes();
    let reason = attributes.next().expect("report must retain the initial attribute");
    assert!(attributes.next().is_none());
    assert_eq!(reason.name(), "reason");
    assert_eq!(reason.value(), "starting");
}

#[test]
fn component_remains_live_after_aggregator_is_dropped() {
    let aggregator = Aggregator::new(Sink::noop());
    let mut component = aggregator.component("database");
    drop(aggregator);

    let mut publisher = component.publisher();
    wait_for_component(&mut component, Health::Healthy);
    publisher.degraded("slow");
    wait_for_component(&mut component, Health::Degraded);
    drop(publisher);
    wait_for_component(&mut component, Health::Unhealthy);
}

#[test]
fn cancelling_a_pending_change_releases_its_last_waker() {
    struct Task;
    impl Wake for Task {
        fn wake(self: Arc<Self>) {
            panic!("an idle monitor should not wake the cancelled task");
        }
    }
    let root = Aggregator::new(Sink::noop());
    let mut monitor = root.monitor();
    let task = Arc::new(Task);
    let weak = Arc::downgrade(&task);
    {
        let mut changed = pin!(monitor.changed());
        let waker = Waker::from(task);
        assert!(changed.as_mut().poll(&mut Context::from_waker(&waker)).is_pending());
    }
    assert!(weak.upgrade().is_none());
}

#[test]
fn equal_signal_does_not_notify() {
    let aggregator = Aggregator::new(Sink::noop());
    let mut component = aggregator.component("database");
    let mut publisher = component.publisher();
    block_on(component.changed());
    publisher.publish(Health::Degraded, [("a", "1"), ("b", "2")]);
    block_on(component.changed());

    publisher.publish(Health::Degraded, [("b", "2"), ("a", "1")]);
    let mut probe = aggregator.component("probe");
    let _probe_publisher = probe.publisher();
    block_on(probe.changed());
    component.changed().unwrap_pending();
}

#[test]
fn every_handle_has_its_own_cursor() {
    let aggregator = Aggregator::new(Sink::noop());
    let mut monitor = aggregator.monitor();
    let mut monitor_clone = monitor.clone();
    monitor.changed().unwrap_pending();

    let mut component = aggregator.component("database");
    let mut component_clone = component.clone();
    block_on(monitor.changed());
    block_on(monitor_clone.changed());
    let _publisher = component.publisher();

    block_on(monitor.changed());
    block_on(monitor_clone.changed());
    block_on(component.changed());
    block_on(component_clone.changed());
    monitor.changed().unwrap_pending();
    assert_eq!(monitor.state(), Health::Healthy);
}

#[test]
fn relocated_handles_observe_updates_from_the_original_publisher() {
    let aggregator = Aggregator::new(Sink::noop());
    let component = aggregator.component("database");
    let mut publisher = component.publisher();
    let mut root = aggregator.clone();
    let mut subsystem = component;
    let mut monitor = aggregator.monitor();
    let source = ThreadBuilder::default().build(thread::current().id());
    thread::scope(|scope| {
        let (ready, relocated) = mpsc::channel();
        let observer = scope.spawn(move || {
            let destination = ThreadBuilder::default().build(thread::current().id());
            root.relocate(Some(&source), &destination);
            subsystem.relocate(Some(&source), &destination);
            monitor.relocate(Some(&source), &destination);
            ready.send(()).unwrap();
            wait_for_monitor(&mut monitor, Health::Degraded);
            wait_for_component(&mut subsystem, Health::Degraded);
            assert_eq!(monitor.state(), Health::Degraded);
        });
        relocated.recv().unwrap();
        publisher.degraded("slow replica");
        observer.join().unwrap();
    });
}

#[test]
fn diagnostics_identify_the_degraded_component_and_publisher_reason() {
    let aggregator = Aggregator::new(Sink::noop());
    let component = aggregator.component("database");
    let mut monitor = aggregator.monitor();
    let mut publisher = component.publisher();
    publisher.degraded("slow replica");
    wait_for_monitor(&mut monitor, Health::Degraded);

    for diagnostic in [
        format!("{aggregator:?}"),
        format!("{monitor:?}"),
        format!("{component:?}"),
        format!("{publisher:?}"),
    ] {
        assert!(diagnostic.contains("Degraded"), "{diagnostic}");
    }
    assert!(format!("{component:?}").contains("database"));
    assert!(format!("{publisher:?}").contains("slow replica"));

    let report = block_on(component.report());
    let (signal, publishers) = report.signals(Health::Degraded).next().unwrap();
    assert_eq!(publishers, 1);
    let mut attributes = signal.attributes();
    let reason = attributes.next().expect("degraded shorthand must publish one reason");
    assert!(attributes.next().is_none());
    assert_eq!(reason.name(), "reason");
    assert_eq!(reason.value(), "slow replica");
}

#[test]
fn monitor_keeps_reading_reports_after_the_aggregator_handle_drops() {
    let aggregator = Aggregator::new(Sink::noop());
    let mut monitor = aggregator.monitor();
    let component = aggregator.component("database");
    let mut publisher = component.publisher();
    wait_for_monitor(&mut monitor, Health::Healthy);
    let mut waiting_monitor = aggregator.monitor();
    let mut changed = pin!(waiting_monitor.changed());
    changed.as_mut().unwrap_pending();
    drop(aggregator);

    publisher.publish(Health::Degraded, [("reason", Box::<str>::from("slow replica"))]);
    block_on(changed.as_mut());
    block_on(async {
        while monitor.state() != Health::Degraded {
            monitor.changed().await;
        }
        let report = monitor.reports().await.next().expect("database remains registered");
        let (signal, count) = report
            .signals(Health::Degraded)
            .next()
            .expect("report includes the degraded publisher");
        assert_eq!(report.name(), "database");
        assert_eq!(count, 1);
        let mut attributes = signal.attributes();
        let reason = attributes.next().expect("expected a single diagnostic attribute");
        assert!(attributes.next().is_none());
        assert_eq!(reason.name(), "reason");
        assert_eq!(reason.value(), "slow replica");
    });
}

#[test]
#[cfg_attr(panic = "abort", ignore = "requires a worker panic to unwind without terminating the process")]
fn readiness_fails_closed_when_a_consumer_panics_on_the_worker() {
    use std::cell::OnceCell;

    struct Observe(Monitor, mpsc::Sender<Health>);
    impl Wake for Observe {
        fn wake(self: Arc<Self>) {
            self.1.send(self.0.state()).unwrap();
        }
    }

    struct PanickingWake(mpsc::Sender<mpsc::Receiver<()>>);
    impl Wake for PanickingWake {
        fn wake(self: Arc<Self>) {
            // This sender disconnects only after the worker unwinds and its thread exits.
            thread_local! { static ALIVE: OnceCell<mpsc::Sender<()>> = const { OnceCell::new() }; }
            ALIVE.with(|alive| {
                alive.get_or_init(|| {
                    let (sender, exited) = mpsc::channel();
                    self.0.send(exited).unwrap();
                    sender
                });
            });
            panic!("simulated consumer failure");
        }
    }

    let aggregator = Aggregator::new(Sink::noop());
    let startup_monitor = aggregator.monitor();
    let component = aggregator.component("database");
    let mut publisher = component.publisher();
    // A turn for a different component runs only after the database startup broadcast returns.
    let mut probe = aggregator.component("probe");
    let _probe_publisher = probe.publisher();
    block_on(probe.changed());
    assert_eq!(startup_monitor.state(), Health::Healthy);
    let monitor = aggregator.monitor();
    let observer_monitor = monitor.clone();
    let mut waiting_monitor = monitor.clone();
    let mut changed = pin!(waiting_monitor.changed());
    let (entered, panic_started) = mpsc::channel();
    let waker = Waker::from(Arc::new(PanickingWake(entered)));
    assert!(changed.as_mut().poll(&mut Context::from_waker(&waker)).is_pending());

    let (woke, states) = mpsc::channel();
    let observer = Waker::from(Arc::new(Observe(observer_monitor, woke)));
    let mut cursor = aggregator.component("database");
    let mut pending = Box::pin(cursor.changed());
    assert!(pending.as_mut().poll(&mut Context::from_waker(&observer)).is_pending());

    // Both signals are healthy: the worker failure, not the published state, must close readiness.
    publisher.publish(Health::Healthy, [("revision", "2")]);
    let exited = receive(&panic_started).unwrap();
    assert_eq!(receive(&states), Ok(Health::Unhealthy));
    assert_eq!(receive(&exited), Err(mpsc::RecvTimeoutError::Disconnected));
    assert_eq!(monitor.state(), Health::Unhealthy);
    assert_eq!(component.state(), Health::Unhealthy);
    assert!(pending.as_mut().poll(&mut Context::from_waker(&observer)).is_ready());
    drop(pending);
    cursor.changed().unwrap_pending();
    assert_eq!(block_on(component.report()).state(), Health::Healthy);
}

#[test]
fn reports_select_signals_by_state() {
    let aggregator = Aggregator::new(Sink::noop());
    let monitor = aggregator.monitor();
    let mut database = aggregator.component("database");
    let mut cache = aggregator.component("cache");
    let mut primary = database.publisher();
    let mut replica = database.publisher();
    let _cache = cache.publisher();
    primary.degraded("slow");
    replica.degraded("slow");
    wait_for_component(&mut cache, Health::Healthy);

    let report = block_on(async {
        loop {
            let report = database.report().await;
            if report.publisher_count(Health::Degraded) == 2 {
                break report;
            }
            database.changed().await;
        }
    });
    let names: HashSet<_> = block_on(monitor.reports()).map(|report| report.name().to_owned()).collect();
    assert_eq!(names, HashSet::from(["database".to_owned(), "cache".to_owned()]));

    assert!(format!("{report:?}").contains("slow"));

    // First read the detailed signals after the component no longer has either publisher.
    drop((primary, replica));
    wait_for_component(&mut database, Health::Unhealthy);
    assert_eq!(report.state(), Health::Degraded);
    assert_eq!(report.publisher_count(Health::Degraded), 2);
    assert_eq!(report.signals(Health::Degraded).next().unwrap().1, 2);
    assert_eq!(report.signals(Health::Degraded).count(), 1);
    assert_eq!(report.signals(Health::Healthy).count(), 0);

    let report = block_on(cache.report());
    assert_eq!(report.publisher_count(Health::Healthy), 1);
    assert_eq!(report.signals(Health::Healthy).count(), 1);
}

#[test]
fn a_burst_of_publishers_yields_to_another_component_and_converges() {
    let aggregator = Aggregator::new(Sink::noop());
    let mut blocker = aggregator.component("observer");
    let mut trigger = blocker.publisher();
    block_on(blocker.changed());
    let (entered, waiting) = mpsc::channel();
    let (release, resume) = mpsc::channel();
    let waker = Waker::from(Arc::new(BlockingWake {
        entered,
        release: Mutex::new(resume),
    }));
    let mut changed = pin!(blocker.changed());
    assert!(changed.as_mut().poll(&mut Context::from_waker(&waker)).is_pending());
    trigger.degraded("pause");
    receive(&waiting).unwrap();

    let mut component = aggregator.component("clients");
    let publishers: Vec<_> = (0..512).map(|_| component.publisher()).collect();
    let mut probe = aggregator.component("probe");
    let (probe_entered, probe_waiting) = mpsc::channel();
    let (probe_release, probe_resume) = mpsc::channel();
    let probe_waker = Waker::from(Arc::new(BlockingWake {
        entered: probe_entered,
        release: Mutex::new(probe_resume),
    }));
    let _probe_publisher = probe.publisher();
    let mut probe_changed = pin!(probe.changed());
    assert!(probe_changed.as_mut().poll(&mut Context::from_waker(&probe_waker)).is_pending());
    release.send(()).unwrap();
    receive(&probe_waiting).unwrap();
    let count_at_probe = block_on(component.report()).publisher_count(Health::Healthy);
    probe_release.send(()).unwrap();
    assert!(count_at_probe < publishers.len(), "a burst must yield to other pending components");
    block_on(async {
        while component.report().await.publisher_count(Health::Healthy) != publishers.len() {
            component.changed().await;
        }
    });
}

#[test]
fn concurrent_publishers_converge_to_latest_signals() {
    let aggregator = Aggregator::new(Sink::noop());
    let mut monitor = aggregator.monitor();
    let mut component = aggregator.component("database");
    let workers: Vec<_> = (0..4)
        .map(|_| {
            let mut publisher = component.publisher();
            thread::spawn(move || {
                for round in 0..1_000 {
                    if round % 2 == 0 {
                        publisher.degraded("flapping");
                    } else {
                        publisher.publish(Health::Unhealthy, Vec::<Attribute>::new());
                    }
                }
                publisher.publish(Health::Healthy, Vec::<Attribute>::new());
                publisher
            })
        })
        .collect();
    let publishers: Vec<_> = workers.into_iter().map(|worker| worker.join().unwrap()).collect();

    let report = block_on(async {
        loop {
            let report = component.report().await;
            if report.publisher_count(Health::Healthy) == publishers.len() {
                break report;
            }
            component.changed().await;
        }
    });
    wait_for_monitor(&mut monitor, Health::Healthy);
    assert_eq!(report.publisher_count(Health::Healthy), publishers.len());

    drop(publishers);
    wait_for_monitor(&mut monitor, Health::Unhealthy);
}
