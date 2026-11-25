// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Native writer creation boundary, replaced by memory-backed writers in tests.

use std::fs::File;
use std::io::{self, Write};
use std::path::Path;

#[cfg(any(test, feature = "rolling_file"))]
use crate::FileLimit;

/// Opens native output writers; tests substitute the registration/I/O boundary.
#[cfg_attr(
    any(test, feature = "test-util"),
    fakeable::fakeable(fake_impl = crate::file_writer::fakes::FakeFileWriterOpener)
)]
#[derive(Debug, Clone)]
pub(crate) struct FileWriterOpener {}

#[cfg_attr(any(test, feature = "test-util"), fakeable::fakeable)]
impl FileWriterOpener {
    pub(crate) fn new() -> Self {
        Self {}
    }

    #[expect(
        clippy::unused_self,
        reason = "Instance methods allow fakeable to substitute writer creation in tests"
    )]
    pub(crate) fn open_file(&self, path: &Path, overwrite: bool) -> io::Result<Box<dyn Write + Send>> {
        let file = File::options()
            .create(true)
            .write(true)
            .append(!overwrite)
            .truncate(overwrite)
            .open(path)?;
        Ok(Box::new(file))
    }

    #[cfg(any(test, feature = "rolling_file"))]
    #[expect(
        clippy::unused_self,
        reason = "Instance methods allow fakeable to substitute writer creation in tests"
    )]
    #[expect(
        clippy::unnecessary_wraps,
        reason = "Matches the fallible writer-open boundary, including injected open failures"
    )]
    pub(crate) fn open_rolling(
        &self,
        path: &Path,
        max_file_size: usize,
        file_limit: FileLimit,
        compression: bool,
    ) -> io::Result<Box<dyn Write + Send>> {
        use file_rotate::compression::Compression;
        use file_rotate::suffix::AppendTimestamp;
        use file_rotate::{ContentLimit, FileRotate};

        let suffix = AppendTimestamp::default(file_limit.into());
        let limit = ContentLimit::BytesSurpassed(max_file_size);
        let compression = if compression { Compression::OnRotate(0) } else { Compression::None };
        Ok(Box::new(FileRotate::new(path, suffix, limit, compression, None)))
    }
}

/// Memory-backed replacement for the files that destinations open.
#[cfg(any(test, feature = "test-util"))]
pub mod fakes {
    use std::io::{self, Cursor, Error, ErrorKind, Seek, SeekFrom, Write};
    use std::path::{Path, PathBuf};

    use performables::arc::Arc;
    use performables::sync::mutex::Mutex;

    #[cfg(any(test, feature = "rolling_file"))]
    use crate::FileLimit;

    /// Records how a destination opens its output and keeps that output in memory.
    ///
    /// Pass it to a destination's `writer_opener` method in place of the native
    /// file system. Clones and all writers that it opens share one stream,
    /// regardless of the path. The fake does not rotate or compress output.
    #[derive(Debug, Clone)]
    pub struct FakeFileWriterOpener {
        contents: Arc<Mutex<Cursor<Vec<u8>>>>,
        calls: Arc<Mutex<Vec<OpenCall>>>,
        open_error: Option<ErrorKind>,
    }

    impl FakeFileWriterOpener {
        /// Creates a fake whose output starts with `contents`, as an existing file does.
        #[must_use]
        pub fn new(contents: &str) -> Self {
            Self {
                contents: Arc::new(Mutex::new(Cursor::new(contents.as_bytes().to_vec()))),
                calls: Arc::new(Mutex::new(Vec::new())),
                open_error: None,
            }
        }

        /// Makes each open attempt fail with an I/O error of this kind.
        ///
        /// The fake still records the attempt in [`calls`](Self::calls).
        #[must_use]
        pub fn open_error(mut self, error: ErrorKind) -> Self {
            self.open_error = Some(error);
            self
        }

        /// Returns the current output as text.
        ///
        /// Invalid UTF-8 sequences become `U+FFFD`.
        #[must_use]
        pub fn output(&self) -> String {
            String::from_utf8_lossy(self.contents.lock().get_ref()).into_owned()
        }

        /// Returns the open attempts, oldest first.
        #[must_use]
        pub fn calls(&self) -> Vec<OpenCall> {
            self.calls.lock().clone()
        }

        pub(crate) fn open_file(&self, path: &Path, overwrite: bool) -> io::Result<Box<dyn Write + Send>> {
            self.calls.lock().push(OpenCall::File {
                path: path.to_path_buf(),
                overwrite,
            });
            self.open_writer(overwrite)
        }

        #[cfg(any(test, feature = "rolling_file"))]
        pub(crate) fn open_rolling(
            &self,
            path: &Path,
            max_file_size: usize,
            file_limit: FileLimit,
            compression: bool,
        ) -> io::Result<Box<dyn Write + Send>> {
            self.calls.lock().push(OpenCall::Rolling {
                path: path.to_path_buf(),
                max_file_size,
                file_limit,
                compression,
            });
            self.open_writer(false)
        }

        fn open_writer(&self, overwrite: bool) -> io::Result<Box<dyn Write + Send>> {
            if let Some(error) = self.open_error {
                return Err(Error::new(error, "injected file open failure"));
            }
            let mut contents = self.contents.lock();
            if overwrite {
                contents.get_mut().clear();
                contents.set_position(0);
            } else {
                contents.seek(SeekFrom::End(0))?;
            }
            Ok(Box::new(MemoryWriter(Arc::clone(&self.contents))))
        }
    }

    /// An open attempt and the options that the destination passed with it.
    ///
    /// The fake records these options but does not apply rotation or compression.
    #[derive(Debug, Clone, PartialEq, Eq)]
    #[non_exhaustive]
    pub enum OpenCall {
        /// A single-file destination opened `path`.
        File {
            /// The configured log file path.
            path: PathBuf,
            /// Whether the destination truncates existing contents.
            overwrite: bool,
        },
        /// A rolling destination opened `path`.
        #[cfg(any(test, feature = "rolling_file"))]
        Rolling {
            /// The configured active log file path.
            path: PathBuf,
            /// The configured rotation size limit in bytes.
            max_file_size: usize,
            /// The configured retention limit for rotated files.
            file_limit: FileLimit,
            /// Whether the destination compresses rotated files.
            compression: bool,
        },
    }

    /// Writes directly to the opener's shared stream; flush has no filesystem effect.
    struct MemoryWriter(Arc<Mutex<Cursor<Vec<u8>>>>);

    impl Write for MemoryWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().write(buf)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.0.lock().flush()
        }
    }
}
