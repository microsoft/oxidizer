// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! MCS queue with flat combining, following ds/combininglock.h.
//! Work stays on the waiting caller's stack; no waiter allocation is necessary.
//! Unlike a mutex, the holder can execute a run of queued range operations
//! without moving the buddy working set between processors for each request.

use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering};

const WAITING: u32 = 0;
const DONE: u32 = 1;
const HEAD: u32 = 2;
const SLEEPING: u32 = 3;

struct Node<T> {
    status: AtomicU32,
    next: AtomicPtr<Self>,
    invoke: unsafe fn(*mut Self, *mut T),
}

#[repr(C)]
struct Work<T, F, R> {
    node: Node<T>,
    action: UnsafeCell<Option<F>>,
    result: UnsafeCell<MaybeUninit<R>>,
}

struct AbortOnUnwind;

fn take_action<F>(action: &mut Option<F>) -> F {
    action.take().unwrap_or_else(|| crate::abort::abort())
}

impl Drop for AbortOnUnwind {
    fn drop(&mut self) {
        if std::thread::panicking() {
            // An unwinding combiner must not leave pointers to departed stacks
            // in the queue. GlobalAlloc also forbids unwinding.
            crate::abort::abort();
        }
    }
}

pub(crate) struct Combining<T> {
    flag: AtomicBool,
    last: AtomicPtr<Node<T>>,
    value: UnsafeCell<T>,
}

// SAFETY: Exactly one flag holder or handed-off queue head accesses value.
// Actions/results crossing threads are constrained to Send by with().
unsafe impl<T: Send> Sync for Combining<T> {}

impl<T: Send> Combining<T> {
    pub(crate) const fn new(value: T) -> Self {
        Self {
            flag: AtomicBool::new(false),
            last: AtomicPtr::new(std::ptr::null_mut()),
            value: UnsafeCell::new(value),
        }
    }

    pub(crate) fn with<R: Send, F: FnOnce(&mut T) -> R + Send>(&self, action: F) -> R {
        unsafe fn invoke<T, F: FnOnce(&mut T) -> R, R>(node: *mut Node<T>, value: *mut T) {
            let work = node.cast::<Work<T, F, R>>();
            // SAFETY: repr(C) puts Node first; the caller keeps Work alive until DONE.
            let action = unsafe { (*work).action.get() };
            // SAFETY: Only this invocation takes the stored FnOnce.
            let action = take_action(unsafe { &mut *action });
            // SAFETY: The same live Work owns its result cell.
            let result = unsafe { (*work).result.get() };
            // SAFETY: This invocation is the cell's only writer.
            let result = unsafe { &mut *result };
            // SAFETY: The combiner owns the exclusive value lease.
            result.write(action(unsafe { &mut *value }));
        }
        let _abort = AbortOnUnwind;
        if self.last.load(Ordering::Relaxed).is_null() && !self.flag.swap(true, Ordering::Acquire) {
            // SAFETY: Successfully acquiring the flag grants exclusive value access.
            let result = action(unsafe { &mut *self.value.get() });
            self.flag.store(false, Ordering::Release);
            return result;
        }
        let mut work = Work {
            node: Node {
                status: AtomicU32::new(WAITING),
                next: AtomicPtr::new(std::ptr::null_mut()),
                invoke: invoke::<T, F, R>,
            },
            action: UnsafeCell::new(Some(action)),
            result: UnsafeCell::new(MaybeUninit::uninit()),
        };
        // SAFETY: Work remains stationary and live until attach observes its
        // completion. FnOnce and result storage are behind UnsafeCell.
        unsafe { self.attach(&raw mut work.node) };
        // SAFETY: attach acquired DONE after the only invocation wrote the result.
        let result = unsafe { &*work.result.get() };
        // SAFETY: The initialized result is moved out exactly once.
        unsafe { result.assume_init_read() }
    }

    unsafe fn wake(node: *mut Node<T>, status: u32) {
        // SAFETY: The node's waiting caller cannot leave before this release.
        // HAL wake uses a numeric wait key rather than a Rust reference, so a
        // caller departing immediately after the exchange is safe.
        let address = unsafe { &raw const (*node).status };
        // SAFETY: The waiting caller keeps this atomic live until the exchange.
        if unsafe { (*address).swap(status, Ordering::AcqRel) } == SLEEPING {
            crate::hal::wake_one(address.cast());
        }
    }

    unsafe fn wait(node: *mut Node<T>) {
        // SAFETY: This is the waiting caller's own live stack node. Only its
        // status atomic is concurrently modified by a combiner.
        let status = unsafe { &(*node).status };
        for _ in 0..100 {
            if status.load(Ordering::Acquire) != WAITING {
                return;
            }
            crate::hal::pause();
        }
        if status
            .compare_exchange(WAITING, SLEEPING, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            crate::hal::wait(status, SLEEPING);
        }
    }

    unsafe fn attach(&self, node: *mut Node<T>) {
        let previous = self.last.swap(node, Ordering::AcqRel);
        if previous.is_null() {
            while self.flag.swap(true, Ordering::Acquire) {
                while self.flag.load(Ordering::Relaxed) {
                    crate::hal::pause();
                }
            }
        } else {
            // SAFETY: The predecessor cannot leave until its successor is linked.
            unsafe { (*previous).next.store(node, Ordering::Release) };
            // SAFETY: node is this caller's live stack storage.
            unsafe { Self::wait(node) };
            // SAFETY: The caller still owns its stack node.
            if unsafe { (*node).status.load(Ordering::Acquire) } == DONE {
                return;
            }
        }
        let mut current = node;
        loop {
            // SAFETY: This queued node stays live until its DONE notification.
            crate::hal::prefetch(unsafe { (*current).next.load(Ordering::Acquire) } as usize);
            // SAFETY: Publication initialized the node's invocation function.
            let invoke = unsafe { (*current).invoke };
            // SAFETY: This combiner owns value and invokes the node exactly once.
            unsafe { invoke(current, self.value.get()) };
            // SAFETY: current has not yet received DONE.
            let successor = unsafe { (*current).next.load(Ordering::Acquire) };
            if successor.is_null() {
                break;
            }
            // SAFETY: The acquired successor lets the predecessor depart.
            unsafe { Self::wake(current, DONE) };
            current = successor;
        }
        if self
            .last
            .compare_exchange(current, std::ptr::null_mut(), Ordering::Release, Ordering::Relaxed)
            .is_ok()
        {
            // SAFETY: Invocation completed and no publisher references current.
            unsafe { Self::wake(current, DONE) };
            self.flag.store(false, Ordering::Release);
            return;
        }
        // SAFETY: A stalled publisher still owns current's successor link.
        while unsafe { (*current).next.load(Ordering::Relaxed) }.is_null() {
            crate::hal::pause();
        }
        // SAFETY: current stays live until DONE; acquire observes its successor.
        let successor = unsafe { (*current).next.load(Ordering::Acquire) };
        // Transfer lock ownership before releasing current, whose stack can depart.
        // SAFETY: The acquired successor is live and receives the value lease.
        unsafe { Self::wake(successor, HEAD) };
        // SAFETY: No subsequent operation dereferences current.
        unsafe { Self::wake(current, DONE) };
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};

    use super::*;

    #[test]
    fn unwinding_action_aborts_without_leaving_a_queue_owner() {
        crate::abort::assert_aborts("combining::tests::unwinding_action_aborts_without_leaving_a_queue_owner", || {
            Combining::new(0).with(|_| panic!("unwind a combining action"));
        });
    }

    #[test]
    fn consumed_action_aborts_instead_of_running_twice() {
        crate::abort::assert_aborts("combining::tests::consumed_action_aborts_instead_of_running_twice", || {
            take_action::<fn()>(&mut None);
        });
        let mut action = Some(|| 123);
        assert_eq!(take_action(&mut action)(), 123);
        assert!(action.is_none());
    }

    #[test]
    fn linked_successors_complete_in_queue_order() {
        struct State {
            last: *const AtomicPtr<Node<Self>>,
            invocations: usize,
        }
        // SAFETY: This exclusive fixture has no concurrent value accesses,
        // and the last pointer stays valid until both nodes complete.
        unsafe impl Send for State {}
        unsafe fn increment(node: *mut Node<State>, value: *mut State) {
            // SAFETY: attach grants this invocation the exclusive value lease.
            let state = unsafe { &mut *value };
            state.invocations += 1;
            // SAFETY: Both nodes remain live until attach finishes.
            let successor = unsafe { &*node }.next.load(Ordering::Acquire);
            if !successor.is_null() {
                // SAFETY: The fixture's queue remains alive throughout attach.
                unsafe { &*state.last }.store(successor, Ordering::Release);
            }
        }
        let mut shared = Combining::new(State {
            last: std::ptr::null(),
            invocations: 0,
        });
        shared.value.get_mut().last = &raw const shared.last;
        let mut successor = Node {
            status: AtomicU32::new(WAITING),
            next: AtomicPtr::new(std::ptr::null_mut()),
            invoke: increment,
        };
        let mut head = Node {
            status: AtomicU32::new(WAITING),
            next: AtomicPtr::new(&raw mut successor),
            invoke: increment,
        };
        // SAFETY: Both stack nodes remain stationary and alive until attach
        // completes; this exclusive fixture has no concurrent queue publisher.
        unsafe { shared.attach(&raw mut head) };
        assert_eq!(head.status.load(Ordering::Acquire), DONE);
        assert_eq!(successor.status.load(Ordering::Acquire), DONE);
        assert!(shared.last.load(Ordering::Acquire).is_null());
        assert!(!shared.flag.load(Ordering::Acquire));
        assert_eq!(shared.with(|value| value.invocations), 2);
    }

    #[test]
    fn stalled_publisher_receives_the_head_lease_before_predecessor_completion() {
        struct State {
            last: *const AtomicPtr<Node<Self>>,
            successor: *mut Node<Self>,
            published: std::sync::mpsc::Sender<()>,
            invocations: usize,
        }
        // SAFETY: The fixture's scoped threads keep both nodes and the queue
        // alive; shared accesses use atomics and State has one value lease.
        unsafe impl Send for State {}
        unsafe fn publish(node: *mut Node<State>, value: *mut State) {
            // SAFETY: attach gives this invocation exclusive access to State.
            let state = unsafe { &mut *value };
            state.invocations += 1;
            // SAFETY: The scoped publisher keeps the successor alive.
            unsafe { &*state.last }.store(state.successor, Ordering::Release);
            // The successor is published but deliberately not linked yet.
            // SAFETY: The caller keeps its node alive throughout invocation.
            assert!(unsafe { &*node }.next.load(Ordering::Acquire).is_null());
            state.published.send(()).unwrap();
        }
        let (published, ready) = std::sync::mpsc::channel();
        let mut successor = Node {
            status: AtomicU32::new(WAITING),
            next: AtomicPtr::new(std::ptr::null_mut()),
            invoke: publish,
        };
        let mut shared = Combining::new(State {
            last: std::ptr::null(),
            successor: &raw mut successor,
            published,
            invocations: 0,
        });
        shared.value.get_mut().last = &raw const shared.last;
        let head = Node {
            status: AtomicU32::new(WAITING),
            next: AtomicPtr::new(std::ptr::null_mut()),
            invoke: publish,
        };
        std::thread::scope(|scope| {
            let head_next = &head.next;
            let successor_address = (&raw const successor) as usize;
            scope.spawn(move || {
                ready.recv().unwrap();
                // Hold the publication gap open while attach reaches its
                // successor-link wait, rather than relying on random contention.
                std::thread::sleep(std::time::Duration::from_millis(50));
                head_next.store(successor_address as *mut Node<State>, Ordering::Release);
            });
            // SAFETY: The scope keeps the publisher and both stationary nodes
            // alive until attach transfers the lease and completes the head.
            unsafe { shared.attach((&raw const head).cast_mut()) };
        });
        assert_eq!(head.status.load(Ordering::Acquire), DONE);
        assert_eq!(successor.status.load(Ordering::Acquire), HEAD);
        assert!(shared.flag.load(Ordering::Acquire));
        assert_eq!(shared.last.load(Ordering::Acquire), &raw mut successor);
        assert_eq!(shared.value.get_mut().invocations, 1);
    }

    #[test]
    fn contended_combining_executes_every_action_once() {
        let shared = Arc::new(Combining::new(0usize));
        let barrier = Arc::new(Barrier::new(16));
        #[expect(
            clippy::needless_collect,
            reason = "All workers must be spawned before any join, or the 16-worker barrier deadlocks"
        )]
        let handles = (0..16)
            .map(|_| {
                let shared = Arc::clone(&shared);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    (0..20000)
                        .map(|_| {
                            shared.with(|value| {
                                let old = *value;
                                *value += 1;
                                old
                            })
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect::<Vec<_>>();
        let mut observed = handles.into_iter().flat_map(|handle| handle.join().unwrap()).collect::<Vec<_>>();
        observed.sort_unstable();
        assert_eq!(observed, (0..320_000).collect::<Vec<_>>());
        assert_eq!(shared.with(|value| *value), 320_000);
    }
}
