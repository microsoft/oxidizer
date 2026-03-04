use autoresolve_macros::resolvable;

struct Bar;
struct Foo;

#[resolvable]
impl Foo {
    fn new() -> Bar {
        Bar
    }
}

fn main() {}
