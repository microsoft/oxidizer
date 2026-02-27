// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fs::{File, Metadata, Permissions};
use std::io::{Error, ErrorKind, Result, Write as _};
use std::path::{Path, PathBuf};

use bytesbuf::BytesView;
use bytesbuf::mem::{Memory, MemoryShared};
use sync_thunk::{Thunker, thunk};

use crate::io_helpers::read_into_bytesbuf;
use crate::path_utils::safe_join;
use crate::shared_memory::SharedMemory;

const DEFAULT_FALLBACK_SIZE: usize = 8192;

/// A capability representing access to a directory on the filesystem.
///
/// All paths used with a `Directory` are relative to the directory it represents.
/// Path components that would escape the directory (such as `..` at the root) are
/// rejected, enforcing capability-based access control.
#[derive(Debug)]
pub struct Directory {
    base_path: PathBuf,
    thunker: Thunker,
}

impl Directory {
    pub(crate) fn new(base_path: PathBuf, thunker: Thunker) -> Self {
        Self { base_path, thunker }
    }

    pub(crate) fn thunker(&self) -> &Thunker {
        &self.thunker
    }

    #[thunk(from = self.thunker)]
    pub(crate) async fn open_std_file(&self, path: &Path, opts: std::fs::OpenOptions) -> Result<File> {
        let full_path = safe_join(&self.base_path, path)?;
        opts.open(&full_path)
    }

    /// Opens a subdirectory, returning a new [`Directory`] capability scoped to it.
    ///
    /// The returned `Directory` restricts all operations to the subdirectory and
    /// its descendants. This is the primary mechanism for narrowing capabilities
    /// in the capability-based access model.
    ///
    /// The given `path` is relative to this directory.
    ///
    /// # Errors
    ///
    /// Returns an error if the path does not exist, is not a directory, or
    /// if the process lacks permission to access it.
    #[inline]
    pub async fn open_dir(&self, path: impl AsRef<Path>) -> Result<Self> {
        self.open_dir_impl(path.as_ref()).await
    }

    #[thunk(from = self.thunker)]
    async fn open_dir_impl(&self, path: &Path) -> Result<Self> {
        let full_path = safe_join(&self.base_path, path)?;
        let metadata = std::fs::metadata(&full_path)?;
        if !metadata.is_dir() {
            return Err(Error::new(ErrorKind::NotADirectory, "path is not a directory"));
        }
        Ok(Self {
            base_path: full_path,
            thunker: self.thunker.clone(),
        })
    }

    /// Returns the canonical, absolute form of a path with all intermediate
    /// components normalized and symbolic links resolved.
    ///
    /// The given `path` is relative to this directory.
    ///
    /// # Errors
    ///
    /// This function will return an error if `path` does not exist or if any
    /// component in the path is not a directory (when used as an intermediate
    /// component).
    #[inline]
    pub async fn canonicalize(&self, path: impl AsRef<Path>) -> Result<PathBuf> {
        self.canonicalize_impl(path.as_ref()).await
    }

    #[thunk(from = self.thunker)]
    async fn canonicalize_impl(&self, path: &Path) -> Result<PathBuf> {
        let full_path = safe_join(&self.base_path, path)?;
        std::fs::canonicalize(&full_path)
    }

    /// Copies the contents of one file to another. This function will also
    /// copy the permission bits of the original file to the destination file.
    /// This function will overwrite the contents of the destination. Note that
    /// if `src` and `dst` both point to the same file, then the file will
    /// likely get truncated by this operation.
    ///
    /// On success, the total number of bytes copied is returned.
    ///
    /// The `src` path is relative to this directory, while the `dst` path is
    /// relative to `dst_dir`.
    ///
    /// # Errors
    ///
    /// This function will return an error if the source file does not exist,
    /// if the user lacks permissions to read the source or write the
    /// destination, or if any other I/O error occurs.
    #[inline]
    pub async fn copy(&self, src: impl AsRef<Path>, dst_dir: &Self, dst: impl AsRef<Path>) -> Result<u64> {
        self.copy_impl(src.as_ref(), dst_dir, dst.as_ref()).await
    }

    #[thunk(from = self.thunker)]
    async fn copy_impl(&self, src: &Path, dst_dir: &Directory, dst: &Path) -> Result<u64> {
        let src_path = safe_join(&self.base_path, src)?;
        let dst_path = safe_join(&dst_dir.base_path, dst)?;
        std::fs::copy(&src_path, &dst_path)
    }

    /// Creates a new, empty directory at the provided path.
    ///
    /// The given `path` is relative to this directory.
    ///
    /// # Errors
    ///
    /// This function will return an error if the user lacks permissions to
    /// create the directory, if the parent directory of `path` does not exist,
    /// or if `path` already exists.
    #[inline]
    pub async fn create_dir(&self, path: impl AsRef<Path>) -> Result<()> {
        self.create_dir_impl(path.as_ref()).await
    }

    #[thunk(from = self.thunker)]
    async fn create_dir_impl(&self, path: &Path) -> Result<()> {
        let full_path = safe_join(&self.base_path, path)?;
        std::fs::create_dir(&full_path)
    }

    /// Recursively creates a directory and all of its parent components if
    /// they are missing.
    ///
    /// The given `path` is relative to this directory.
    ///
    /// # Errors
    ///
    /// This function will return an error if the user lacks permissions to
    /// create any of the directories, or if any other I/O error occurs.
    /// This function will succeed if the full directory path already exists.
    #[inline]
    pub async fn create_dir_all(&self, path: impl AsRef<Path>) -> Result<()> {
        self.create_dir_all_impl(path.as_ref()).await
    }

    #[thunk(from = self.thunker)]
    async fn create_dir_all_impl(&self, path: &Path) -> Result<()> {
        let full_path = safe_join(&self.base_path, path)?;
        std::fs::create_dir_all(&full_path)
    }

    /// Returns `Ok(true)` if the path points at an existing entity.
    ///
    /// This function will traverse symbolic links to query information about
    /// the destination file. The given `path` is relative to this directory.
    ///
    /// Returns `Ok(false)` if the path does not exist or if existence cannot
    /// be determined, and `Err` only on I/O errors unrelated to the existence
    /// of the path.
    ///
    /// # Errors
    ///
    /// This function will return an error only if it encounters an I/O error
    /// that is not related to whether the path exists.
    #[inline]
    pub async fn exists(&self, path: impl AsRef<Path>) -> Result<bool> {
        self.exists_impl(path.as_ref()).await
    }

    #[thunk(from = self.thunker)]
    async fn exists_impl(&self, path: &Path) -> Result<bool> {
        let full_path = safe_join(&self.base_path, path)?;
        full_path.try_exists()
    }

    /// Creates a new hard link on the filesystem.
    ///
    /// The `dst` file will be a link pointing to the `src` file. Neither path
    /// may be a directory. The `src` path is relative to this directory, while
    /// the `dst` path is relative to `dst_dir`.
    ///
    /// # Errors
    ///
    /// This function will return an error if `src` does not exist, if either
    /// path is a directory, if the user lacks permissions, or if the source
    /// and destination are on different filesystems.
    #[inline]
    pub async fn hard_link(&self, src: impl AsRef<Path>, dst_dir: &Self, dst: impl AsRef<Path>) -> Result<()> {
        self.hard_link_impl(src.as_ref(), dst_dir, dst.as_ref()).await
    }

    #[thunk(from = self.thunker)]
    async fn hard_link_impl(&self, src: &Path, dst_dir: &Directory, dst: &Path) -> Result<()> {
        let src_path = safe_join(&self.base_path, src)?;
        let dst_path = safe_join(&dst_dir.base_path, dst)?;
        std::fs::hard_link(&src_path, &dst_path)
    }

    /// Given a path, queries the file system to get information about a file,
    /// directory, etc.
    ///
    /// This function will traverse symbolic links to query information about
    /// the destination file. The given `path` is relative to this directory.
    ///
    /// # Errors
    ///
    /// This function will return an error if the path does not exist, if the
    /// user lacks permissions to query metadata, or if any other I/O error
    /// occurs.
    #[inline]
    pub async fn metadata(&self, path: impl AsRef<Path>) -> Result<Metadata> {
        self.metadata_impl(path.as_ref()).await
    }

    #[thunk(from = self.thunker)]
    async fn metadata_impl(&self, path: &Path) -> Result<Metadata> {
        let full_path = safe_join(&self.base_path, path)?;
        std::fs::metadata(&full_path)
    }

    /// Reads the entire contents of a file into a [`BytesView`].
    ///
    /// This is a convenience function for opening a file, reading it, and
    /// closing it. Returns the contents allocated from the default memory
    /// pool. The given `path` is relative to this directory.
    ///
    /// # Errors
    ///
    /// This function will return an error if the file does not exist, if the
    /// user lacks permissions to read it, or if any other I/O error occurs.
    #[inline]
    pub async fn read(&self, path: impl AsRef<Path>) -> Result<BytesView> {
        self.read_impl(path.as_ref(), SharedMemory::global()).await
    }

    /// Reads the entire contents of a file into a [`BytesView`] using the
    /// specified memory provider.
    ///
    /// This allows the caller to control buffer allocation, enabling
    /// zero-copy transfers to other subsystems that share the same memory
    /// provider. The given `path` is relative to this directory.
    ///
    /// # Errors
    ///
    /// This function will return an error if the file does not exist, if the
    /// user lacks permissions to read it, or if any other I/O error occurs.
    #[inline]
    pub async fn read_with_memory(&self, path: impl AsRef<Path>, memory: impl MemoryShared) -> Result<BytesView> {
        self.read_impl(path.as_ref(), SharedMemory::new(memory)).await
    }

    #[thunk(from = self.thunker)]
    async fn read_impl(&self, path: &Path, memory: SharedMemory) -> Result<BytesView> {
        let full_path = safe_join(&self.base_path, path)?;
        let mut file = File::open(&full_path)?;
        let len = usize::try_from(file.metadata()?.len()).unwrap_or(usize::MAX);
        if len > 0 {
            let mut buf = memory.reserve(len);
            let mut total = 0;
            while total < len {
                let n = read_into_bytesbuf(&mut file, &mut buf, len - total)?;
                if n == 0 {
                    break;
                }
                total += n;
            }
            Ok(buf.consume_all())
        } else {
            // Zero-length metadata (e.g. procfs files that report 0 size but
            // have content). Try one read; if empty, return immediately without
            // wasting a large allocation.
            let mut buf = memory.reserve(DEFAULT_FALLBACK_SIZE);
            let n = read_into_bytesbuf(&mut file, &mut buf, DEFAULT_FALLBACK_SIZE)?;
            if n == 0 {
                return Ok(buf.consume_all());
            }
            // The file has content despite reporting 0 length — keep reading.
            loop {
                if buf.remaining_capacity() == 0 {
                    buf.reserve(DEFAULT_FALLBACK_SIZE, &memory);
                }
                let to_read = buf.remaining_capacity();
                let n = read_into_bytesbuf(&mut file, &mut buf, to_read)?;
                if n == 0 {
                    break;
                }
            }
            Ok(buf.consume_all())
        }
    }

    /// Returns a [`ReadDir`](crate::read_dir::ReadDir) over the entries
    /// within a directory.
    ///
    /// The given `path` is relative to this directory.
    ///
    /// # Errors
    ///
    /// This function will return an error if the path does not exist, if the
    /// path is not a directory, if the user lacks permissions to read the
    /// directory, or if any other I/O error occurs.
    #[inline]
    pub async fn read_dir(&self, path: impl AsRef<Path>) -> Result<crate::read_dir::ReadDir> {
        self.read_dir_impl(path.as_ref()).await
    }

    #[thunk(from = self.thunker)]
    async fn read_dir_impl(&self, path: &Path) -> Result<crate::read_dir::ReadDir> {
        let full_path = safe_join(&self.base_path, path)?;
        let read_dir = std::fs::read_dir(&full_path)?;
        Ok(crate::read_dir::ReadDir::new(read_dir, self.thunker.clone()))
    }

    /// Reads a symbolic link, returning the file that the link points to.
    ///
    /// The given `path` is relative to this directory.
    ///
    /// # Errors
    ///
    /// This function will return an error if the path does not exist, if the
    /// path is not a symbolic link, if the user lacks permissions, or if any
    /// other I/O error occurs.
    #[inline]
    pub async fn read_link(&self, path: impl AsRef<Path>) -> Result<PathBuf> {
        self.read_link_impl(path.as_ref()).await
    }

    #[thunk(from = self.thunker)]
    async fn read_link_impl(&self, path: &Path) -> Result<PathBuf> {
        let full_path = safe_join(&self.base_path, path)?;
        std::fs::read_link(&full_path)
    }

    /// Reads the entire contents of a file into a string.
    ///
    /// This is a convenience function for opening a file, reading it, and
    /// closing it. The given `path` is relative to this directory.
    ///
    /// # Errors
    ///
    /// This function will return an error if the file does not exist, if the
    /// user lacks permissions to read it, if the file's contents are not
    /// valid UTF-8, or if any other I/O error occurs.
    #[inline]
    pub async fn read_to_string(&self, path: impl AsRef<Path>) -> Result<String> {
        self.read_to_string_impl(path.as_ref()).await
    }

    #[thunk(from = self.thunker)]
    async fn read_to_string_impl(&self, path: &Path) -> Result<String> {
        let full_path = safe_join(&self.base_path, path)?;
        std::fs::read_to_string(&full_path)
    }

    /// Removes an existing, empty directory.
    ///
    /// The given `path` is relative to this directory.
    ///
    /// # Errors
    ///
    /// This function will return an error if the path does not exist, if the
    /// directory is not empty, if the user lacks permissions, or if any other
    /// I/O error occurs.
    #[inline]
    pub async fn remove_dir(&self, path: impl AsRef<Path>) -> Result<()> {
        self.remove_dir_impl(path.as_ref()).await
    }

    #[thunk(from = self.thunker)]
    async fn remove_dir_impl(&self, path: &Path) -> Result<()> {
        let full_path = safe_join(&self.base_path, path)?;
        std::fs::remove_dir(&full_path)
    }

    /// Removes a directory at this path, after removing all its contents.
    /// Use carefully!
    ///
    /// The given `path` is relative to this directory.
    ///
    /// # Errors
    ///
    /// This function will return an error if the path does not exist, if the
    /// user lacks permissions to remove the directory or any of its contents,
    /// or if any other I/O error occurs.
    #[inline]
    pub async fn remove_dir_all(&self, path: impl AsRef<Path>) -> Result<()> {
        self.remove_dir_all_impl(path.as_ref()).await
    }

    #[thunk(from = self.thunker)]
    async fn remove_dir_all_impl(&self, path: &Path) -> Result<()> {
        let full_path = safe_join(&self.base_path, path)?;
        std::fs::remove_dir_all(&full_path)
    }

    /// Removes a file from the filesystem.
    ///
    /// There is no guarantee that the file is immediately deleted. The given
    /// `path` is relative to this directory.
    ///
    /// # Errors
    ///
    /// This function will return an error if the path does not exist, if the
    /// user lacks permissions to remove the file, or if any other I/O error
    /// occurs.
    #[inline]
    pub async fn remove_file(&self, path: impl AsRef<Path>) -> Result<()> {
        self.remove_file_impl(path.as_ref()).await
    }

    #[thunk(from = self.thunker)]
    async fn remove_file_impl(&self, path: &Path) -> Result<()> {
        let full_path = safe_join(&self.base_path, path)?;
        std::fs::remove_file(&full_path)
    }

    /// Renames a file or directory to a new name, replacing the original file
    /// if the destination already exists.
    ///
    /// The `src` path is relative to this directory, while the `dst` path is
    /// relative to `dst_dir`.
    ///
    /// # Errors
    ///
    /// This function will return an error if `src` does not exist, if the
    /// user lacks permissions, or if any other I/O error occurs.
    #[inline]
    pub async fn rename(&self, src: impl AsRef<Path>, dst_dir: &Self, dst: impl AsRef<Path>) -> Result<()> {
        self.rename_impl(src.as_ref(), dst_dir, dst.as_ref()).await
    }

    #[thunk(from = self.thunker)]
    async fn rename_impl(&self, src: &Path, dst_dir: &Directory, dst: &Path) -> Result<()> {
        let src_path = safe_join(&self.base_path, src)?;
        let dst_path = safe_join(&dst_dir.base_path, dst)?;
        std::fs::rename(&src_path, &dst_path)
    }

    /// Changes the permissions found on a file or a directory.
    ///
    /// The given `path` is relative to this directory.
    ///
    /// # Errors
    ///
    /// This function will return an error if the path does not exist, if the
    /// user lacks permissions to change the file permissions, or if any other
    /// I/O error occurs.
    #[inline]
    pub async fn set_permissions(&self, path: impl AsRef<Path>, perms: Permissions) -> Result<()> {
        self.set_permissions_impl(path.as_ref(), perms).await
    }

    #[thunk(from = self.thunker)]
    async fn set_permissions_impl(&self, path: &Path, perms: Permissions) -> Result<()> {
        let full_path = safe_join(&self.base_path, path)?;
        std::fs::set_permissions(&full_path, perms)
    }

    /// Creates a new symbolic link on the filesystem.
    ///
    /// The `link` path will be a symbolic link pointing to the `original`
    /// path. Both paths are relative to this directory.
    ///
    /// # Errors
    ///
    /// This function will return an error if `link` already exists, if the
    /// user lacks permissions, or if any other I/O error occurs.
    #[inline]
    pub async fn symlink(&self, original: impl AsRef<Path>, link: impl AsRef<Path>) -> Result<()> {
        self.symlink_impl(original.as_ref(), link.as_ref()).await
    }

    #[thunk(from = self.thunker)]
    async fn symlink_impl(&self, original: &Path, link: &Path) -> Result<()> {
        let original_path = safe_join(&self.base_path, original)?;
        let link_path = safe_join(&self.base_path, link)?;
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&original_path, &link_path)
        }
        #[cfg(windows)]
        {
            if std::fs::metadata(&original_path).map(|m| m.is_dir()).unwrap_or(false) {
                std::os::windows::fs::symlink_dir(&original_path, &link_path)
            } else {
                std::os::windows::fs::symlink_file(&original_path, &link_path)
            }
        }
    }

    /// Queries the metadata about a file without following symlinks.
    ///
    /// If the path is a symlink, metadata for the symlink itself is returned
    /// rather than the file it points to. The given `path` is relative to
    /// this directory.
    ///
    /// # Errors
    ///
    /// This function will return an error if the path does not exist, if the
    /// user lacks permissions to query metadata, or if any other I/O error
    /// occurs.
    #[inline]
    pub async fn symlink_metadata(&self, path: impl AsRef<Path>) -> Result<Metadata> {
        self.symlink_metadata_impl(path.as_ref()).await
    }

    #[thunk(from = self.thunker)]
    async fn symlink_metadata_impl(&self, path: &Path) -> Result<Metadata> {
        let full_path = safe_join(&self.base_path, path)?;
        std::fs::symlink_metadata(&full_path)
    }

    /// Writes the entire contents of a [`BytesView`] as a file.
    ///
    /// This is a convenience function for creating or truncating a file,
    /// writing to it, and closing it. The given `path` is relative to this
    /// directory.
    ///
    /// # Errors
    ///
    /// This function will return an error if the parent directory of the path
    /// does not exist, if the user lacks permissions to write the file, or if
    /// any other I/O error occurs.
    #[inline]
    pub async fn write(&self, path: impl AsRef<Path>, contents: BytesView) -> Result<()> {
        self.write_impl(path.as_ref(), contents).await
    }

    #[thunk(from = self.thunker)]
    async fn write_impl(&self, path: &Path, contents: BytesView) -> Result<()> {
        let full_path = safe_join(&self.base_path, path)?;
        let mut file = File::create(&full_path)?;
        let mut contents = contents;
        while !contents.is_empty() {
            let slice = contents.first_slice();
            let len = slice.len();
            file.write_all(slice)?;
            contents.advance(len);
        }
        Ok(())
    }

    /// Writes a byte slice as the entire contents of a file.
    ///
    /// This is a convenience wrapper around [`write`](Self::write) for
    /// callers working with `&[u8]` data. The given `path` is relative to
    /// this directory.
    ///
    /// # Errors
    ///
    /// This function will return an error if the parent directory of the path
    /// does not exist, if the user lacks permissions to write the file, or if
    /// any other I/O error occurs.
    #[inline]
    pub async fn write_slice(&self, path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> Result<()> {
        self.write_slice_impl(path.as_ref(), contents.as_ref()).await
    }

    #[thunk(from = self.thunker)]
    async fn write_slice_impl(&self, path: &Path, contents: &[u8]) -> Result<()> {
        let full_path = safe_join(&self.base_path, path)?;
        std::fs::write(&full_path, contents)
    }
}
