use std::sync::Arc;

use autoresolve_macros::resolvable;

#[derive(Clone)]
struct Base;

#[derive(Clone)]
struct Alpha {
    beta: Arc<Beta>,
}

#[resolvable]
impl Alpha {
    fn new(beta: &Beta) -> Self {
        Self {
            beta: Arc::new(beta.clone()),
        }
    }
}

#[derive(Clone)]
struct Beta {
    alpha: Arc<Alpha>,
}

#[resolvable]
impl Beta {
    fn new(alpha: &Alpha) -> Self {
        Self {
            alpha: Arc::new(alpha.clone()),
        }
    }
}

fn main() {
    let base_val = Base;
    let mut resolver = autoresolve::resolver!(ResolverBase, base_val: Base);
    let _alpha = resolver.get::<Alpha>();
}
