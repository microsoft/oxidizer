// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use syn::{Path, parse_quote};

use crate::attrs::ContainerAttrs;

pub(crate) fn append_module(root_path: &Path, module: syn::Ident) -> Path {
    let mut root = root_path.clone();
    root.segments.push(module.into());
    root
}

pub(crate) fn resolve_crate_root<'a>(container: &'a ContainerAttrs, root_path: &'a Path) -> &'a Path {
    container.internity_crate.as_ref().unwrap_or(root_path)
}

/// Resolves the path to the `internity::de` module from the default crate root
/// and the container's optional `#[internity(crate = "...")]` override.
pub(crate) fn resolve_de_root(container: &ContainerAttrs, root_path: &Path) -> Path {
    append_module(resolve_crate_root(container, root_path), parse_quote!(de))
}

/// Resolves the path to the `internity::se` module from the default crate root
/// and the container's optional `#[internity(crate = "...")]` override.
pub(crate) fn resolve_se_root(container: &ContainerAttrs, root_path: &Path) -> Path {
    append_module(resolve_crate_root(container, root_path), parse_quote!(se))
}
