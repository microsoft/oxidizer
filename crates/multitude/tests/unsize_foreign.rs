// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Feature-independent trait-object coercion tests.

use core::any::Any;

use multitude::{Arc, Arena, Box, Rc, coerce};

#[test]
fn any_trait_objects_are_supported_without_the_dst_feature() {
    let arena = Arena::new();

    let boxed: Box<dyn Any> = Box::unsize(arena.alloc_box(7_u32), coerce!(dyn Any));
    assert_eq!(boxed.downcast_ref::<u32>(), Some(&7));

    let rc: Rc<dyn Any> = Rc::unsize(arena.alloc_rc(8_u32), coerce!(dyn Any));
    assert_eq!(rc.downcast_ref::<u32>(), Some(&8));

    let arc: Arc<dyn Any + Send + Sync> = Arc::unsize(arena.alloc_arc(9_u32), coerce!(dyn Any + Send + Sync));
    assert_eq!(arc.downcast_ref::<u32>(), Some(&9));
}
