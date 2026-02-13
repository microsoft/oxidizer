use crate::ResolveFrom;
use crate::resolver::Resolver;

pub struct ResolutionDepsEnd;

pub struct ResolutionDepsNode<H, T>(pub H, pub T);

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

impl<T, H, Rest> ResolutionDeps<T> for ResolutionDepsNode<H, Rest>
where
    H: ResolveFrom<T>,
    Rest: ResolutionDeps<T>,
    T: Send + Sync + 'static,
{
    type Resolved<'a>
        = ResolutionDepsNode<&'a H, Rest::Resolved<'a>>
    where
        Self: 'a,
        T: 'a;
    fn get_private(base: &Resolver<T>) -> Self::Resolved<'_> {
        let tail = Rest::get_private(base);
        let head = base.try_get::<H>().expect("Ensure must have been called before new");
        ResolutionDepsNode(head, tail)
    }

    fn ensure(base: &mut Resolver<T>) {
        base.ensure::<H>();
        Rest::ensure(base);
    }
}
