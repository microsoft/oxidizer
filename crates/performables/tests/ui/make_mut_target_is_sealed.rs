// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::Arc;

use performables::arc::MakeMutTarget;

struct Custom;

impl MakeMutTarget for Custom {
    fn make_mut(arc: &mut Arc<Self>) -> &mut Self {
        Arc::get_mut(arc).unwrap()
    }
}

fn main() {}
