use std::marker::PhantomData;

use crate::ResolveFrom;
use crate::resolver::Resolver;

pub struct ResolutionDepsEnd;

pub struct ResolutionDepsNode<H, S, T>(pub H, pub T, pub PhantomData<S>);

pub trait ResolutionDeps<T>: Send + Sync + 'static {
    type Resolved<'a>
    where
        Self: 'a,
        T: 'a;

    fn ensure(base: &mut Resolver<T>);

    fn get_private(base: &Resolver<T>) -> Self::Resolved<'_>;

    fn get(base: &mut Resolver<T>) -> Self::Resolved<'_> {
        Self::ensure(base);
        Self::get_private(base)
    }
}

impl<T> ResolutionDeps<T> for ResolutionDepsEnd {
    type Resolved<'a>
        = ResolutionDepsEnd
    where
        Self: 'a,
        T: 'a;

    fn ensure(_base: &mut Resolver<T>) {}

    fn get_private(_base: &Resolver<T>) -> Self::Resolved<'_> {
        ResolutionDepsEnd
    }
}

impl<T, S, H, Rest> ResolutionDeps<T> for ResolutionDepsNode<H, S, Rest>
where
    T: AsRef<S>,
    H: ResolveFrom<S>,
    Rest: ResolutionDeps<T>,
    T: Send + Sync + 'static,
    S: Send + Sync + 'static,
{
    type Resolved<'a>
        = ResolutionDepsNode<&'a H, S, Rest::Resolved<'a>>
    where
        Self: 'a,
        T: 'a;
    fn get_private(base: &Resolver<T>) -> Self::Resolved<'_> {
        /*let tail = Rest::get_private(base);
        let head = base.try_get::<H>().expect("Ensure must have been called before new");
        ResolutionDepsNode(head, tail, PhantomData)*/
        todo!()
    }

    fn ensure(base: &mut Resolver<T>) {
        base.ensure::<H, S>();
        Rest::ensure(base);
    }
}
