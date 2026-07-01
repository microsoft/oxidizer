// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Vendored `google.api` annotation protos.
//!
//! Reading `google.api.http` annotations off a proto requires the annotation
//! schema to be available when the consumer compiles their `.proto` files. To
//! save consumers from sourcing the googleapis protos themselves, faithful
//! (schema-preserving) copies of `google/api/http.proto` and
//! `google/api/annotations.proto` are embedded here (both Apache-2.0, from
//! <https://github.com/googleapis/googleapis>).
//!
//! Call [`write_annotation_protos`] from a `build.rs` to materialize them under
//! an include directory, then add that directory to the `protox` include path.

use std::path::Path;
use std::{fs, io};

/// The vendored `google/api/http.proto` source.
///
/// # Examples
///
/// ```
/// use rest_over_grpc_build::HTTP_PROTO;
///
/// assert!(HTTP_PROTO.contains("package google.api;"));
/// assert!(HTTP_PROTO.contains("message HttpRule"));
/// ```
pub const HTTP_PROTO: &str = include_str!("proto/google/api/http.proto");

/// The vendored `google/api/annotations.proto` source (defines the
/// `google.api.http` method-options extension).
///
/// # Examples
///
/// ```
/// use rest_over_grpc_build::ANNOTATIONS_PROTO;
///
/// assert!(ANNOTATIONS_PROTO.contains("package google.api;"));
/// assert!(ANNOTATIONS_PROTO.contains("extend google.protobuf.MethodOptions"));
/// ```
pub const ANNOTATIONS_PROTO: &str = include_str!("proto/google/api/annotations.proto");

/// Writes the vendored annotation protos under `include_root`, creating
/// `include_root/google/api/{http,annotations}.proto`.
///
/// Pass `include_root` to `protox` as an include directory so that a `.proto`
/// importing `google/api/annotations.proto` resolves against these copies.
///
/// # Errors
///
/// Returns any I/O error encountered creating the directories or files.
///
/// # Examples
///
/// ```no_run
/// use std::path::PathBuf;
///
/// use rest_over_grpc_build::write_annotation_protos;
///
/// let include_root =
///     PathBuf::from(std::env::var("OUT_DIR").expect("Cargo sets OUT_DIR for build scripts"))
///         .join("proto_include");
/// write_annotation_protos(&include_root)?;
/// # Ok::<(), std::io::Error>(())
/// ```
pub fn write_annotation_protos(include_root: &Path) -> io::Result<()> {
    let api_dir = include_root.join("google").join("api");
    fs::create_dir_all(&api_dir)?;
    fs::write(api_dir.join("http.proto"), HTTP_PROTO)?;
    fs::write(api_dir.join("annotations.proto"), ANNOTATIONS_PROTO)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::{env, fs, process};

    use super::{ANNOTATIONS_PROTO, HTTP_PROTO, write_annotation_protos};

    static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

    fn scratch_dir(name: &str) -> PathBuf {
        let suffix = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("rest_over_grpc_build_tests")
            .join(format!("{name}-{}-{suffix}", process::id()));
        fs::create_dir_all(&dir).expect("scratch directory is created under target");
        dir
    }

    #[test]
    fn writes_vendored_annotation_protos() {
        let dir = scratch_dir("annotations");

        write_annotation_protos(&dir).expect("annotation protos are written");

        let api_dir = dir.join("google").join("api");
        assert_eq!(
            fs::read_to_string(api_dir.join("http.proto")).expect("http proto is readable"),
            HTTP_PROTO
        );
        assert_eq!(
            fs::read_to_string(api_dir.join("annotations.proto")).expect("annotations proto is readable"),
            ANNOTATIONS_PROTO
        );
    }
}
