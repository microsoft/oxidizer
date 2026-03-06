use crate::resolver::Resolver;

/// A type that serves as the root configuration for a [`Resolver`].
///
/// This trait is automatically implemented by the `#[base]` proc macro. It provides the
/// logic to construct a [`Resolver`] from the base struct by inserting all root types.
pub trait BaseType: Sized + Send + Sync + 'static {
    /// Consumes this base value and returns a fully-initialized [`Resolver`] with all
    /// root types pre-inserted.
    fn into_resolver(self) -> Resolver<Self>;
}
