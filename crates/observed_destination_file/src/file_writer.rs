// Copyright (c) Microsoft Corporation.

//! Native writer creation boundary, replaced by memory-backed writers in tests.

#![cfg_attr(
    test,
    expect(
        unexpected_cfgs,
        reason = "fakeable emits a test-util feature check, but the macro and fakes are only compiled under cfg(test)"
    )
)]

use std::fs::File;
use std::io::{self, Seek, SeekFrom, Write};
use std::path::Path;

#[cfg(feature = "rolling_file")]
use crate::FileLimit;

/// Opens native output writers; tests substitute the registration/I/O boundary.
#[cfg_attr(test, fakeable::fakeable(fake_impl = crate::file_writer::fakes::FakeFileWriterOpener))]
#[derive(Debug, Clone)]
pub(crate) struct FileWriterOpener {}

#[cfg_attr(test, fakeable::fakeable)]
impl FileWriterOpener {
    pub(crate) fn new() -> Self {
        Self {}
    }

    #[expect(
        clippy::unused_self,
        reason = "Instance methods allow fakeable to substitute writer creation in tests"
    )]
    pub(crate) fn open_file(&self, path: &Path, overwrite: bool) -> io::Result<Box<dyn Write + Send>> {
        let mut file = File::options().create(true).write(true).truncate(overwrite).open(path)?;
        if !overwrite {
            file.seek(SeekFrom::End(0))?;
        }
        Ok(Box::new(file))
    }

    #[cfg(feature = "rolling_file")]
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

#[cfg(test)]
pub(crate) mod fakes {
    use std::io::{self, Cursor, Error, ErrorKind, Seek, SeekFrom, Write};
    use std::path::{Path, PathBuf};

    use performables::arc::Arc;
    use performables::sync::mutex::Mutex;

    #[cfg(feature = "rolling_file")]
    use crate::FileLimit;

    /// Memory-backed opener for registration and worker assertions.
    /// Clones and all opened writers share one stream, regardless of path.
    #[derive(Debug, Clone)]
    pub(crate) struct FakeFileWriterOpener {
        contents: Arc<Mutex<Cursor<Vec<u8>>>>,
        calls: Arc<Mutex<Vec<OpenCall>>>,
        open_error: Option<ErrorKind>,
    }

    impl FakeFileWriterOpener {
        pub(crate) fn new(contents: &str) -> Self {
            Self {
                contents: Arc::new(Mutex::new(Cursor::new(contents.as_bytes().to_vec()))),
                calls: Arc::new(Mutex::new(Vec::new())),
                open_error: None,
            }
        }

        pub(crate) fn open_error(mut self, error: ErrorKind) -> Self {
            self.open_error = Some(error);
            self
        }

        pub(crate) fn output(&self) -> String {
            String::from_utf8(self.contents.lock().get_ref().clone()).unwrap()
        }

        pub(crate) fn calls(&self) -> Vec<OpenCall> {
            self.calls.lock().clone()
        }

        pub(crate) fn open_file(&self, path: &Path, overwrite: bool) -> io::Result<Box<dyn Write + Send>> {
            self.calls.lock().push(OpenCall::File {
                path: path.to_path_buf(),
                overwrite,
            });
            self.open_writer(overwrite)
        }

        #[cfg(feature = "rolling_file")]
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

    /// Recorded options only; the fake does not execute rotation or compression.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) enum OpenCall {
        File {
            path: PathBuf,
            overwrite: bool,
        },
        #[cfg(feature = "rolling_file")]
        Rolling {
            path: PathBuf,
            max_file_size: usize,
            file_limit: FileLimit,
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
