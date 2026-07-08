// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Configuration for [`ServiceDefinition::from_fds`](crate::build::ServiceDefinition::from_fds).
///
/// Controls which services to decode and how message types are resolved to Rust
/// paths.
///
/// # Examples
///
/// ```
/// use rest_over_grpc::build::DescriptorOptions;
///
/// let options = DescriptorOptions::new()
///     .package(".library")
///     .extern_path(".google.protobuf.Empty", "::prost_types::Empty");
/// ```
#[derive(Debug, Default, Clone)]
pub struct DescriptorOptions {
    packages: Vec<String>,
    extern_paths: Vec<(String, String)>,
}

impl DescriptorOptions {
    /// Creates default options: all annotated services and no type overrides.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Restricts decoding to services whose fully-qualified proto name is covered
    /// by `prefix` (a proto path such as `".library"`, matched on segment
    /// boundaries). Call repeatedly to include several packages; if never called,
    /// every annotated service is included.
    #[must_use]
    pub fn package(mut self, prefix: impl Into<String>) -> Self {
        self.packages.push(prefix.into());
        self
    }

    /// Maps a proto type path to an external Rust type path, consulted before the
    /// default package-relative resolution (longest proto prefix wins).
    ///
    /// Use this for well-known types or messages generated in another crate, e.g.
    /// `.extern_path(".google.protobuf.Empty", "::prost_types::Empty")` or a whole
    /// package `.extern_path(".google.protobuf", "::prost_types")`.
    #[must_use]
    pub fn extern_path(mut self, proto_path: impl Into<String>, rust_path: impl Into<String>) -> Self {
        self.extern_paths.push((proto_path.into(), rust_path.into()));
        self
    }

    /// The proto package prefixes to include (empty means all services).
    pub(crate) fn packages(&self) -> &[String] {
        &self.packages
    }

    /// The proto-path → Rust-path type overrides.
    pub(crate) fn extern_paths(&self) -> &[(String, String)] {
        &self.extern_paths
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_empty() {
        let options = DescriptorOptions::new();
        assert!(options.packages().is_empty());
        assert!(options.extern_paths().is_empty());
    }

    #[test]
    fn builders_accumulate_and_are_readable() {
        let options = DescriptorOptions::new()
            .package(".library")
            .package(".other")
            .extern_path(".google.protobuf.Empty", "::prost_types::Empty");

        assert_eq!(options.packages(), &[".library".to_owned(), ".other".to_owned()]);
        assert_eq!(
            options.extern_paths(),
            &[(".google.protobuf.Empty".to_owned(), "::prost_types::Empty".to_owned())]
        );
    }

    #[test]
    fn extern_path_preserves_prior_state() {
        // The builder must return the updated `Self`, not a fresh default.
        let options = DescriptorOptions::new().package(".library").extern_path("a", "b");
        assert_eq!(options.packages(), &[".library".to_owned()]);
        assert_eq!(options.extern_paths(), &[("a".to_owned(), "b".to_owned())]);
    }
}
