// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `ThreadAware` impl for [`tokio::sync::mpsc::UnboundedSender`](UnboundedSender) (tokio 1.x).
//!
//! Enable with the `tokio` Cargo feature.
//!
//! `UnboundedSender<T>` is a cheaply-cloneable, thread-safe handle onto a
//! channel's shared queue: sending through it never touches thread-local
//! state, so relocating the handle itself is a no-op. This is unrelated to
//! the messages flowing through the channel — those are ordinary values of
//! `T` moving via `send`/`recv`, not something `relocate` reaches into.

use ::tokio::sync::mpsc::UnboundedSender;

use crate::ThreadAware;
use crate::affinity::Affinity;

impl<T: Send> ThreadAware for UnboundedSender<T> {
    fn relocate(&mut self, _source: Option<Affinity>, _destination: Affinity) {}
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use ::tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use crate::ThreadAware;
    use crate::affinity::pinned_affinities;

    assert_impl_all!(UnboundedSender<u32>: ThreadAware, Send, Sync, Clone);

    assert_not_impl_any!(UnboundedSender<Rc<u32>>: ThreadAware, Send);

    #[test]
    fn unbounded_sender_relocate_is_noop_and_stays_usable() {
        let affinities = pinned_affinities(&[2]);
        let (mut tx, mut rx) = unbounded_channel::<u32>();

        tx.send(1).expect("receiver is still alive");

        tx.relocate(Some(affinities[0]), affinities[1]);

        let tx_clone = tx.clone();
        tx_clone.send(2).expect("receiver is still alive");
        tx.send(3).expect("receiver is still alive");
        drop(tx);
        drop(tx_clone);

        assert_eq!(rx.try_recv().expect("first message"), 1);
        assert_eq!(rx.try_recv().expect("second message"), 2);
        assert_eq!(rx.try_recv().expect("third message"), 3);
        assert!(rx.try_recv().is_err(), "channel must be drained and closed");
    }
}
