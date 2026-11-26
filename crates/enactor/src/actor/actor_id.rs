// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Identity of a spawned actor.

use std::fmt;
use std::num::NonZeroU64;
use std::sync::atomic::{AtomicU64, Ordering};

// This is the one deliberate static in the crate. Ids must be unique across
// every `ActorRuntime`, and code often builds one runtime per task, so a
// counter per runtime would repeat ids. `tokio::task::Id` uses the same design.
static NEXT_ACTOR_ID: AtomicU64 = AtomicU64::new(1);

/// Identifies one spawned actor.
///
/// Each spawn gets an id that no other actor in the process has. Clones of an
/// [`ActorRef`](crate::ActorRef) share the id of their actor.
///
/// You can compare, hash, sort, and print an id, but you cannot create one.
/// The sort order has no meaning beyond a stable order. [`Display`](fmt::Display)
/// prints an id as `actor-<n>`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct ActorId(NonZeroU64);

impl ActorId {
    /// Allocates an id that no other actor in the process has.
    pub(crate) fn next() -> Self {
        let id = NEXT_ACTOR_ID.fetch_add(1, Ordering::Relaxed);
        Self(NonZeroU64::new(id).expect("ids start at 1, and the counter reaches 0 again only after 2^64 spawns"))
    }
}

impl fmt::Display for ActorId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "actor-{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU64;

    use super::ActorId;

    #[test]
    fn display_prints_the_actor_prefix_and_number() {
        let id = ActorId(NonZeroU64::new(42).unwrap());

        assert_eq!(id.to_string(), "actor-42");
    }
}
