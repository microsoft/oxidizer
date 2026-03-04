use crate::resolve_deps::{ResolutionDeps, ResolutionDepsEnd, ResolutionDepsNode};

pub trait ResolveFrom<T>: Send + Sync + 'static {
    type Inputs: ResolutionDeps<T>;

    fn new(inputs: <Self::Inputs as ResolutionDeps<T>>::Resolved<'_>) -> Self;
}

impl<T> ResolveFrom<T> for T
where
    T: Clone + Send + Sync + 'static,
{
    type Inputs = ResolutionDepsNode<T, ResolutionDepsEnd>;

    fn new(input: <Self::Inputs as ResolutionDeps<T>>::Resolved<'_>) -> T {
        let ResolutionDepsNode(value, ResolutionDepsEnd) = input;
        value.clone()
    }
}

pub trait ResolveFrom2<T1, T2>: Send + Sync + 'static {
    type Inputs1: ResolutionDeps<T1>;
    type Inputs2: ResolutionDeps<T2>;

    fn new(
        inputs1: <Self::Inputs1 as ResolutionDeps<T1>>::Resolved<'_>,
        inputs2: <Self::Inputs2 as ResolutionDeps<T2>>::Resolved<'_>,
    ) -> Self;
}
