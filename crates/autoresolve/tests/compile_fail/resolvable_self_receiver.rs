use autoresolve_macros::resolvable;

#[derive(Clone)]
struct Foo;

#[resolvable]
impl Foo {
    fn new(&self) -> Self {
        self.clone()
    }
}

fn main() {}
