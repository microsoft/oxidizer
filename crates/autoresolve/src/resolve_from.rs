use crate::resolve_deps::{ResolutionDeps, ResolutionDepsEnd, ResolutionDepsNode};

pub trait ResolveFrom<T>: Send + Sync + 'static {
    type Inputs: ResolutionDeps<T>;

    fn new_resolved_from(inputs: <Self::Inputs as ResolutionDeps<T>>::Resolved<'_>) -> Self;
}

impl<T> ResolveFrom<T> for T
where
    T: Clone + Send + Sync + 'static,
{
    type Inputs = ResolutionDepsNode<T, ResolutionDepsEnd>;

    fn new_resolved_from(input: <Self::Inputs as ResolutionDeps<T>>::Resolved<'_>) -> T {
        let ResolutionDepsNode(value, ResolutionDepsEnd) = input;
        value.clone()
    }
}
