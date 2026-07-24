// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Arena-backed strings: the various flavors and their use cases.

#![allow(clippy::unwrap_used, reason = "example code")]
#![allow(clippy::missing_panics_doc, reason = "example code")]
#![allow(clippy::std_instead_of_core, reason = "example uses std::thread/HashMap")]

use std::collections::HashMap;
use std::thread;

use multitude::{Arc, Arena, Box};
fn main() {
    let arena = Arena::new();

    let s1: Arc<str> = arena.alloc_str_arc("Hello, world!");
    println!("ArenaArcStr: {} (len={})", &*s1, s1.len());

    let mut builder = arena.alloc_string();
    builder.push_str("built ");
    builder.push_str("up ");
    builder.push_str("incrementally");
    println!("ArenaString (mutable): {}", builder.as_str());
    let s2: Box<str> = builder.into_boxed_str();
    println!("frozen: {}", &*s2);

    let name = "Alice";
    let s3 = multitude::strings::format!(in &arena, "format! says hi, {name}");
    println!("format!: {}", &*s3);

    let shared: Arc<str> = arena.alloc_str_arc("shared across threads");
    let cloned = shared.clone();
    let h = thread::spawn(move || cloned.len());
    println!("ArenaArcStr: {} (len from thread: {})", &*shared, h.join().unwrap());

    let mut counts: HashMap<Arc<str>, i32> = HashMap::new();
    let _ = counts.insert(arena.alloc_str_arc("alpha"), 1);
    let _ = counts.insert(arena.alloc_str_arc("beta"), 2);
    println!("map alpha: {:?}", counts.get("alpha"));
}
