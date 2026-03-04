use autoresolve_macros::resolvable;

struct Foo;

#[resolvable]
impl Foo {
    fn new() {}
}

fn main() {}
