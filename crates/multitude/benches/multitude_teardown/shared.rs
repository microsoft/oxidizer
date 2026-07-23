// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use multitude::{Alloc, Arena};

pub(crate) const SMALL: usize = 1;
pub(crate) const MEDIUM: usize = 32;
pub(crate) const LARGE: usize = 1_000;

const PAYLOAD_BYTES: usize = 64;
type Payload = [u64; PAYLOAD_BYTES / size_of::<u64>()];

pub(crate) struct StandardState<const N: usize>(Option<[std::boxed::Box<Payload>; N]>);

fn payload(index: usize) -> Payload {
    [u64::try_from(index).expect("benchmark allocation counts fit in u64"); PAYLOAD_BYTES / size_of::<u64>()]
}

pub(crate) fn standard_state<const N: usize>() -> StandardState<N> {
    let values = std::array::from_fn(|index| std::boxed::Box::new(payload(index)));
    StandardState(Some(values))
}

pub(crate) fn multitude_state<const N: usize>() -> Arena {
    let arena = Arena::builder().with_capacity(N * PAYLOAD_BYTES + 64 * 1024).build();
    for index in 0..N {
        let _ = Alloc::leak(arena.alloc(payload(index)));
    }
    arena
}

pub(crate) fn bumpalo_state<const N: usize>() -> bumpalo::Bump {
    let bump = bumpalo::Bump::with_capacity(N * PAYLOAD_BYTES + 64 * 1024);
    for index in 0..N {
        let _ = bump.alloc(payload(index));
    }
    bump
}

#[inline(never)]
pub(crate) fn free_standard<const N: usize>(state: &mut StandardState<N>) {
    drop(state.0.take().expect("benchmark setup always provides allocations"));
}

#[inline(never)]
pub(crate) fn reset_multitude(arena: &mut Arena) {
    arena.reset();
}

#[inline(never)]
pub(crate) fn reset_bumpalo(bump: &mut bumpalo::Bump) {
    bump.reset();
}
