use autoresolve_macros::resolvable;

struct Foo;

trait SomeTrait {
    fn new() -> Self;
}

#[resolvable]
impl SomeTrait for Foo {
    fn new() -> Self {
        Foo
    }
}

fn main() {}
