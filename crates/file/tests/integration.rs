// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(clippy::unwrap_used, reason = "Tests use unwrap for brevity")]
#![allow(clippy::missing_panics_doc, reason = "Tests")]
#![allow(clippy::missing_errors_doc, reason = "Tests")]
#![allow(unused_results, reason = "Tests")]
#![allow(clippy::must_use_candidate, reason = "Tests")]
#![allow(clippy::needless_pass_by_value, reason = "Tests")]
#![allow(clippy::string_slice, reason = "Tests")]
#![allow(missing_docs, reason = "Tests")]
#![allow(clippy::assertions_on_result_states, reason = "Tests use assert!(x.is_err()) for clarity")]
#![allow(clippy::std_instead_of_core, reason = "Tests prefer std imports")]
#![allow(clippy::filetype_is_file, reason = "Test intentionally checks is_file()")]

use std::ffi::OsString;
use std::time::{Duration, SystemTime};

use bytesbuf::mem::GlobalPool;
use file::{
    DirBuilder, File, OpenOptions, PositionalFile, ReadOnlyFile, ReadOnlyPositionalFile, Root, SeekFrom, WriteOnlyFile,
    WriteOnlyPositionalFile,
};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn setup() -> (TempDir, file::Directory) {
    let tmp = TempDir::new().unwrap();
    let dir = Root::bind(tmp.path()).await.unwrap();
    (tmp, dir)
}

fn make_view(data: &[u8]) -> bytesbuf::BytesView {
    let mem = GlobalPool::new();
    let mut buf = mem.reserve(data.len());
    buf.put_slice(data);
    buf.consume_all()
}

// ===========================================================================
// Root tests
// ===========================================================================

mod root {
    use super::*;

    #[tokio::test]
    async fn bind_to_valid_directory_succeeds() {
        let tmp = TempDir::new().unwrap();
        let _dir = Root::bind(tmp.path()).await.unwrap();
    }

    #[tokio::test]
    async fn bind_to_non_existent_path_fails() {
        let result = Root::bind("/tmp/__nonexistent_path_12345__").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn bind_to_file_fails() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("a_file.txt");
        std::fs::write(&file_path, b"hello").unwrap();
        let result = Root::bind(&file_path).await;
        assert!(result.is_err());
    }
}

// ===========================================================================
// Directory tests
// ===========================================================================

mod directory {
    use super::*;

    #[tokio::test]
    async fn create_dir_and_exists() {
        let (_tmp, dir) = setup().await;
        dir.create_dir("sub").await.unwrap();
        assert!(dir.exists("sub").await.unwrap());
    }

    #[tokio::test]
    async fn create_dir_all_nested() {
        let (_tmp, dir) = setup().await;
        dir.create_dir_all("a/b/c").await.unwrap();
        assert!(dir.exists("a/b/c").await.unwrap());
    }

    #[tokio::test]
    async fn read_and_write_bytes_view() {
        let (_tmp, dir) = setup().await;
        let data = make_view(b"hello bytes");
        dir.write("file.bin", data).await.unwrap();
        let view = dir.read("file.bin").await.unwrap();
        let mut collected = Vec::new();
        let mut v = view;
        while !v.is_empty() {
            let s = v.first_slice();
            collected.extend_from_slice(s);
            let len = s.len();
            v.advance(len);
        }
        assert_eq!(collected, b"hello bytes");
    }

    #[tokio::test]
    async fn read_to_string_round_trip() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("greeting.txt", b"Good morning").await.unwrap();
        let s = dir.read_to_string("greeting.txt").await.unwrap();
        assert_eq!(s, "Good morning");
    }

    #[tokio::test]
    async fn write_slice_and_read() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("data.txt", b"slice data").await.unwrap();
        let s = dir.read_to_string("data.txt").await.unwrap();
        assert_eq!(s, "slice data");
    }

    #[tokio::test]
    async fn read_with_custom_memory() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("mem.txt", b"custom memory").await.unwrap();
        let mem = GlobalPool::new();
        let view = dir.read_with_memory("mem.txt", mem).await.unwrap();
        let mut collected = Vec::new();
        let mut v = view;
        while !v.is_empty() {
            let s = v.first_slice();
            collected.extend_from_slice(s);
            let len = s.len();
            v.advance(len);
        }
        assert_eq!(collected, b"custom memory");
    }

    #[tokio::test]
    async fn exists_false_for_nonexistent() {
        let (_tmp, dir) = setup().await;
        assert!(!dir.exists("nope.txt").await.unwrap());
    }

    #[tokio::test]
    async fn metadata_returns_info() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("m.txt", b"12345").await.unwrap();
        let md = dir.metadata("m.txt").await.unwrap();
        assert!(md.is_file());
        assert_eq!(md.len(), 5);
    }

    #[tokio::test]
    async fn symlink_metadata() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("orig.txt", b"content").await.unwrap();
        let md = dir.symlink_metadata("orig.txt").await.unwrap();
        assert!(md.is_file());
    }

    #[tokio::test]
    async fn remove_file_works() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("delete_me.txt", b"bye").await.unwrap();
        assert!(dir.exists("delete_me.txt").await.unwrap());
        dir.remove_file("delete_me.txt").await.unwrap();
        assert!(!dir.exists("delete_me.txt").await.unwrap());
    }

    #[tokio::test]
    async fn remove_dir_empty() {
        let (_tmp, dir) = setup().await;
        dir.create_dir("empty").await.unwrap();
        dir.remove_dir("empty").await.unwrap();
        assert!(!dir.exists("empty").await.unwrap());
    }

    #[tokio::test]
    async fn remove_dir_all_recursive() {
        let (_tmp, dir) = setup().await;
        dir.create_dir_all("tree/branch").await.unwrap();
        dir.write_slice("tree/branch/leaf.txt", b"leaf").await.unwrap();
        dir.remove_dir_all("tree").await.unwrap();
        assert!(!dir.exists("tree").await.unwrap());
    }

    #[tokio::test]
    async fn rename_same_dir() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("old.txt", b"data").await.unwrap();
        dir.rename("old.txt", &dir, "new.txt").await.unwrap();
        assert!(!dir.exists("old.txt").await.unwrap());
        let s = dir.read_to_string("new.txt").await.unwrap();
        assert_eq!(s, "data");
    }

    #[tokio::test]
    async fn rename_cross_dir() {
        let (_tmp, dir) = setup().await;
        dir.create_dir("src_dir").await.unwrap();
        dir.create_dir("dst_dir").await.unwrap();
        dir.write_slice("src_dir/f.txt", b"moved").await.unwrap();
        let src = dir.open_dir("src_dir").await.unwrap();
        let dst = dir.open_dir("dst_dir").await.unwrap();
        src.rename("f.txt", &dst, "f.txt").await.unwrap();
        assert!(!src.exists("f.txt").await.unwrap());
        let s = dst.read_to_string("f.txt").await.unwrap();
        assert_eq!(s, "moved");
    }

    #[tokio::test]
    async fn copy_same_dir() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("src.txt", b"copy me").await.unwrap();
        let n = dir.copy("src.txt", &dir, "dst.txt").await.unwrap();
        assert_eq!(n, 7);
        let s = dir.read_to_string("dst.txt").await.unwrap();
        assert_eq!(s, "copy me");
    }

    #[tokio::test]
    async fn copy_cross_dir() {
        let (_tmp, dir) = setup().await;
        dir.create_dir("a").await.unwrap();
        dir.create_dir("b").await.unwrap();
        dir.write_slice("a/f.txt", b"cross").await.unwrap();
        let a = dir.open_dir("a").await.unwrap();
        let b = dir.open_dir("b").await.unwrap();
        a.copy("f.txt", &b, "f.txt").await.unwrap();
        let s = b.read_to_string("f.txt").await.unwrap();
        assert_eq!(s, "cross");
    }

    #[tokio::test]
    async fn hard_link_works() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("original.txt", b"linked").await.unwrap();
        dir.hard_link("original.txt", &dir, "link.txt").await.unwrap();
        let s = dir.read_to_string("link.txt").await.unwrap();
        assert_eq!(s, "linked");
    }

    #[tokio::test]
    async fn canonicalize_returns_path() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("c.txt", b"x").await.unwrap();
        let canon = dir.canonicalize("c.txt").await.unwrap();
        assert!(canon.is_absolute());
    }

    #[tokio::test]
    async fn read_dir_multiple_entries() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("a.txt", b"a").await.unwrap();
        dir.write_slice("b.txt", b"b").await.unwrap();
        dir.write_slice("c.txt", b"c").await.unwrap();
        let mut rd = dir.read_dir(".").await.unwrap();
        let mut names: Vec<OsString> = Vec::new();
        while let Some(entry) = rd.next_entry().await.unwrap() {
            names.push(entry.file_name().to_owned());
        }
        names.sort();
        assert_eq!(
            names,
            vec![OsString::from("a.txt"), OsString::from("b.txt"), OsString::from("c.txt")]
        );
    }

    #[tokio::test]
    async fn read_dir_empty() {
        let (_tmp, dir) = setup().await;
        dir.create_dir("empty").await.unwrap();
        let mut rd = dir.read_dir("empty").await.unwrap();
        assert!(rd.next_entry().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn open_dir_subdirectory() {
        let (_tmp, dir) = setup().await;
        dir.create_dir("sub").await.unwrap();
        dir.write_slice("sub/f.txt", b"inner").await.unwrap();
        let sub = dir.open_dir("sub").await.unwrap();
        let s = sub.read_to_string("f.txt").await.unwrap();
        assert_eq!(s, "inner");
    }

    #[tokio::test]
    async fn path_escape_with_dotdot_rejected() {
        let (_tmp, dir) = setup().await;
        let result = dir.read_to_string("../etc/passwd").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn absolute_path_rejected() {
        let (_tmp, dir) = setup().await;
        let result = dir.read_to_string("/etc/passwd").await;
        assert!(result.is_err());
    }
}

// ===========================================================================
// ReadOnlyFile tests
// ===========================================================================

mod read_only_file {
    use super::*;

    #[tokio::test]
    async fn open_and_read_max() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("r.txt", b"readable").await.unwrap();
        let mut f = ReadOnlyFile::open(&dir, "r.txt").await.unwrap();
        let buf = f.read_max(8192).await.unwrap();
        assert_eq!(buf.len(), 8);
    }

    #[tokio::test]
    async fn read_max_into_bytebuf() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("ram.txt", b"hello world").await.unwrap();
        let mut f = ReadOnlyFile::open(&dir, "ram.txt").await.unwrap();
        let mem = GlobalPool::new();
        let mut buf = mem.reserve(32);
        let n = f.read_max_into_bytebuf(5, &mut buf).await.unwrap();
        assert_eq!(n, 5);
        assert_eq!(buf.len(), 5);
    }

    #[tokio::test]
    async fn read_into_bytebuf() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("rmi.txt", b"more data here").await.unwrap();
        let mut f = ReadOnlyFile::open(&dir, "rmi.txt").await.unwrap();
        let mem = GlobalPool::new();
        let mut buf = mem.reserve(128);
        let n = f.read_into_bytebuf(&mut buf).await.unwrap();
        assert!(n > 0);
    }

    #[tokio::test]
    async fn read_into_slice_works() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("rs.txt", b"slice_test").await.unwrap();
        let mut f = ReadOnlyFile::open(&dir, "rs.txt").await.unwrap();

        let mut buf = [0u8; 5];
        let n = f.read_max_into_slice(5, &mut buf).await.unwrap();
        assert_eq!(n, 5);
        assert_eq!(&buf[..n], b"slice");
    }

    #[tokio::test]
    async fn open_with_memory() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("owm.txt", b"with memory").await.unwrap();
        let mem = GlobalPool::new();
        let mut f = ReadOnlyFile::open_with_memory(&dir, "owm.txt", mem).await.unwrap();
        let buf = f.read_max(8192).await.unwrap();
        assert_eq!(buf.len(), 11);
    }

    #[tokio::test]
    async fn metadata_works() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("md.txt", b"12345678").await.unwrap();
        let f = ReadOnlyFile::open(&dir, "md.txt").await.unwrap();
        let md = f.metadata().await.unwrap();
        assert_eq!(md.len(), 8);
        assert!(md.is_file());
    }

    #[tokio::test]
    async fn seek_stream_position_rewind() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("seek.txt", b"0123456789").await.unwrap();
        let mut f = ReadOnlyFile::open(&dir, "seek.txt").await.unwrap();

        let pos = f.seek(SeekFrom::Start(5)).await.unwrap();
        assert_eq!(pos, 5);

        let pos = f.stream_position().await.unwrap();
        assert_eq!(pos, 5);

        f.rewind().await.unwrap();
        let pos = f.stream_position().await.unwrap();
        assert_eq!(pos, 0);
    }

    #[tokio::test]
    async fn try_clone_shares_state() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("clone.txt", b"clone data").await.unwrap();
        let f = ReadOnlyFile::open(&dir, "clone.txt").await.unwrap();
        let _f2 = f.try_clone().await.unwrap();
        let md = f.metadata().await.unwrap();
        assert_eq!(md.len(), 10);
    }

    #[tokio::test]
    async fn open_nonexistent_fails() {
        let (_tmp, dir) = setup().await;
        let result = ReadOnlyFile::open(&dir, "nope.txt").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn lock_and_unlock() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("lock.txt", b"x").await.unwrap();
        let f = ReadOnlyFile::open(&dir, "lock.txt").await.unwrap();
        f.lock().await.unwrap();
        f.unlock().await.unwrap();
    }

    #[tokio::test]
    async fn lock_shared_works() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("slock.txt", b"x").await.unwrap();
        let f = ReadOnlyFile::open(&dir, "slock.txt").await.unwrap();
        f.lock_shared().await.unwrap();
        f.unlock().await.unwrap();
    }

    #[tokio::test]
    async fn try_lock_and_try_lock_shared() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("tlock.txt", b"x").await.unwrap();
        let f = ReadOnlyFile::open(&dir, "tlock.txt").await.unwrap();
        f.try_lock().await.unwrap();
        f.unlock().await.unwrap();
        f.try_lock_shared().await.unwrap();
        f.unlock().await.unwrap();
    }
}

// ===========================================================================
// WriteOnlyFile tests
// ===========================================================================

mod write_only_file {
    use super::*;

    #[tokio::test]
    async fn create_and_write_bytes_view() {
        let (_tmp, dir) = setup().await;
        let mut f = WriteOnlyFile::create(&dir, "w.txt").await.unwrap();
        let data = make_view(b"written via BytesView");
        f.write(data).await.unwrap();
        drop(f);
        let s = dir.read_to_string("w.txt").await.unwrap();
        assert_eq!(s, "written via BytesView");
    }

    #[tokio::test]
    async fn create_new_succeeds_then_fails() {
        let (_tmp, dir) = setup().await;
        let f = WriteOnlyFile::create_new(&dir, "new.txt").await.unwrap();
        drop(f);
        let result = WriteOnlyFile::create_new(&dir, "new.txt").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn create_with_memory_works() {
        let (_tmp, dir) = setup().await;
        let mem = GlobalPool::new();
        let mut f = WriteOnlyFile::create_with_memory(&dir, "cwm.txt", mem).await.unwrap();
        f.write_slice(b"with memory").await.unwrap();
        drop(f);
        let s = dir.read_to_string("cwm.txt").await.unwrap();
        assert_eq!(s, "with memory");
    }

    #[tokio::test]
    async fn create_new_with_memory_works() {
        let (_tmp, dir) = setup().await;
        let mem = GlobalPool::new();
        let mut f = WriteOnlyFile::create_new_with_memory(&dir, "cnwm.txt", mem).await.unwrap();
        f.write_slice(b"new with mem").await.unwrap();
        drop(f);
        let s = dir.read_to_string("cnwm.txt").await.unwrap();
        assert_eq!(s, "new with mem");
    }

    #[tokio::test]
    async fn write_slice_works() {
        let (_tmp, dir) = setup().await;
        let mut f = WriteOnlyFile::create(&dir, "ws.txt").await.unwrap();
        f.write_slice(b"slice write").await.unwrap();
        drop(f);
        let s = dir.read_to_string("ws.txt").await.unwrap();
        assert_eq!(s, "slice write");
    }

    #[tokio::test]
    async fn set_len_truncate() {
        let (_tmp, dir) = setup().await;
        let mut f = WriteOnlyFile::create(&dir, "slt.txt").await.unwrap();
        f.write_slice(b"1234567890").await.unwrap();
        f.set_len(5).await.unwrap();
        drop(f);
        let s = dir.read_to_string("slt.txt").await.unwrap();
        assert_eq!(s, "12345");
    }

    #[tokio::test]
    async fn set_len_extend() {
        let (_tmp, dir) = setup().await;
        let mut f = WriteOnlyFile::create(&dir, "sle.txt").await.unwrap();
        f.write_slice(b"AB").await.unwrap();
        f.set_len(10).await.unwrap();
        let md = f.metadata().await.unwrap();
        assert_eq!(md.len(), 10);
    }

    #[tokio::test]
    async fn flush_sync_all_sync_data() {
        let (_tmp, dir) = setup().await;
        let mut f = WriteOnlyFile::create(&dir, "sync.txt").await.unwrap();
        f.write_slice(b"sync data").await.unwrap();
        f.flush().await.unwrap();
        f.sync_all().await.unwrap();
        f.sync_data().await.unwrap();
    }

    #[tokio::test]
    async fn set_permissions_works() {
        let (_tmp, dir) = setup().await;
        let f = WriteOnlyFile::create(&dir, "perms.txt").await.unwrap();
        let md = f.metadata().await.unwrap();
        let perms = md.permissions();
        f.set_permissions(perms).await.unwrap();
    }

    #[tokio::test]
    async fn set_modified_works() {
        let (_tmp, dir) = setup().await;
        let f = WriteOnlyFile::create(&dir, "mod.txt").await.unwrap();
        let t = SystemTime::now() - Duration::from_secs(3600);
        f.set_modified(t).await.unwrap();
    }

    #[tokio::test]
    async fn set_times_works() {
        let (_tmp, dir) = setup().await;
        let f = WriteOnlyFile::create(&dir, "times.txt").await.unwrap();
        let now = SystemTime::now();
        let times = file::FileTimes::new().set_modified(now).set_accessed(now);
        f.set_times(times).await.unwrap();
    }

    #[tokio::test]
    async fn seek_stream_position_rewind() {
        let (_tmp, dir) = setup().await;
        let mut f = WriteOnlyFile::create(&dir, "seek_w.txt").await.unwrap();
        f.write_slice(b"0123456789").await.unwrap();
        let pos = f.seek(SeekFrom::Start(5)).await.unwrap();
        assert_eq!(pos, 5);
        let pos = f.stream_position().await.unwrap();
        assert_eq!(pos, 5);
        f.rewind().await.unwrap();
        let pos = f.stream_position().await.unwrap();
        assert_eq!(pos, 0);
    }

    #[tokio::test]
    async fn try_clone_works() {
        let (_tmp, dir) = setup().await;
        let f = WriteOnlyFile::create(&dir, "tc.txt").await.unwrap();
        let _f2 = f.try_clone().await.unwrap();
    }

    #[tokio::test]
    async fn metadata_works() {
        let (_tmp, dir) = setup().await;
        let mut f = WriteOnlyFile::create(&dir, "wmd.txt").await.unwrap();
        f.write_slice(b"12345").await.unwrap();
        let md = f.metadata().await.unwrap();
        assert_eq!(md.len(), 5);
    }
}

// ===========================================================================
// File tests
// ===========================================================================

mod read_write_file {
    use super::*;

    #[tokio::test]
    async fn open_existing_for_rw() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("rw.txt", b"existing").await.unwrap();
        let _f = File::open(&dir, "rw.txt").await.unwrap();
    }

    #[tokio::test]
    async fn create_new_file() {
        let (_tmp, dir) = setup().await;
        let mut f = File::create(&dir, "rw_new.txt").await.unwrap();
        f.write_slice(b"created").await.unwrap();
        drop(f);
        let s = dir.read_to_string("rw_new.txt").await.unwrap();
        assert_eq!(s, "created");
    }

    #[tokio::test]
    async fn create_truncates_existing() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("trunc.txt", b"old data old data").await.unwrap();
        let mut f = File::create(&dir, "trunc.txt").await.unwrap();
        f.write_slice(b"new").await.unwrap();
        drop(f);
        let s = dir.read_to_string("trunc.txt").await.unwrap();
        assert_eq!(s, "new");
    }

    #[tokio::test]
    async fn create_new_fails_on_existing() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("exists.txt", b"x").await.unwrap();
        let result = File::create_new(&dir, "exists.txt").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn create_new_succeeds_on_new() {
        let (_tmp, dir) = setup().await;
        let _f = File::create_new(&dir, "brand_new.txt").await.unwrap();
    }

    #[tokio::test]
    async fn open_with_memory_works() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("owm.txt", b"data").await.unwrap();
        let mem = GlobalPool::new();
        let _f = File::open_with_memory(&dir, "owm.txt", mem).await.unwrap();
    }

    #[tokio::test]
    async fn create_with_memory_works() {
        let (_tmp, dir) = setup().await;
        let mem = GlobalPool::new();
        let mut f = File::create_with_memory(&dir, "cwm.txt", mem).await.unwrap();
        f.write_slice(b"mem data").await.unwrap();
        drop(f);
        let s = dir.read_to_string("cwm.txt").await.unwrap();
        assert_eq!(s, "mem data");
    }

    #[tokio::test]
    async fn create_new_with_memory_works() {
        let (_tmp, dir) = setup().await;
        let mem = GlobalPool::new();
        let mut f = File::create_new_with_memory(&dir, "cnwm.txt", mem).await.unwrap();
        f.write_slice(b"new mem").await.unwrap();
        drop(f);
        let s = dir.read_to_string("cnwm.txt").await.unwrap();
        assert_eq!(s, "new mem");
    }

    #[tokio::test]
    async fn write_then_seek_back_and_read() {
        let (_tmp, dir) = setup().await;
        let mut f = File::create(&dir, "wsb.txt").await.unwrap();
        f.write_slice(b"Hello, World!").await.unwrap();
        f.rewind().await.unwrap();
        let buf = f.read_max(8192).await.unwrap();
        assert_eq!(buf.len(), 13);
    }

    #[tokio::test]
    async fn write_slice_then_read_slice_round_trip() {
        let (_tmp, dir) = setup().await;
        let mut f = File::create(&dir, "slrt.txt").await.unwrap();
        f.write_slice(b"round trip").await.unwrap();
        f.rewind().await.unwrap();
        let mut buf = [0u8; 10];
        let n = f.read_max_into_slice(10, &mut buf).await.unwrap();
        assert_eq!(n, 10);
        assert_eq!(&buf[..n], b"round trip");
    }

    #[tokio::test]
    async fn metadata_and_set_len() {
        let (_tmp, dir) = setup().await;
        let mut f = File::create(&dir, "rwmd.txt").await.unwrap();
        f.write_slice(b"12345").await.unwrap();
        let md = f.metadata().await.unwrap();
        assert_eq!(md.len(), 5);
        f.set_len(3).await.unwrap();
        let md2 = f.metadata().await.unwrap();
        assert_eq!(md2.len(), 3);
    }

    #[tokio::test]
    async fn lock_unlock_cycle() {
        let (_tmp, dir) = setup().await;
        let f = File::create(&dir, "rwlock.txt").await.unwrap();
        f.lock().await.unwrap();
        f.unlock().await.unwrap();
        f.lock_shared().await.unwrap();
        f.unlock().await.unwrap();
        f.try_lock().await.unwrap();
        f.unlock().await.unwrap();
        f.try_lock_shared().await.unwrap();
        f.unlock().await.unwrap();
    }

    #[tokio::test]
    async fn seek_stream_position_rewind() {
        let (_tmp, dir) = setup().await;
        let mut f = File::create(&dir, "rwseek.txt").await.unwrap();
        f.write_slice(b"0123456789").await.unwrap();
        let pos = f.seek(SeekFrom::Start(3)).await.unwrap();
        assert_eq!(pos, 3);
        let pos = f.stream_position().await.unwrap();
        assert_eq!(pos, 3);
        f.rewind().await.unwrap();
        let pos = f.stream_position().await.unwrap();
        assert_eq!(pos, 0);
    }

    #[tokio::test]
    async fn flush_sync_all_sync_data() {
        let (_tmp, dir) = setup().await;
        let mut f = File::create(&dir, "rwsync.txt").await.unwrap();
        f.write_slice(b"sync").await.unwrap();
        f.flush().await.unwrap();
        f.sync_all().await.unwrap();
        f.sync_data().await.unwrap();
    }

    #[tokio::test]
    async fn set_permissions_and_set_modified_and_set_times() {
        let (_tmp, dir) = setup().await;
        let f = File::create(&dir, "rwperms.txt").await.unwrap();
        let md = f.metadata().await.unwrap();
        f.set_permissions(md.permissions()).await.unwrap();
        let t = SystemTime::now() - Duration::from_secs(100);
        f.set_modified(t).await.unwrap();
        let times = file::FileTimes::new().set_modified(SystemTime::now());
        f.set_times(times).await.unwrap();
    }

    #[tokio::test]
    async fn options_returns_open_options() {
        let mut opts = File::options();
        // Just verify it returns an OpenOptions (compiles and is usable)
        let (_tmp, dir) = setup().await;
        dir.write_slice("opts.txt", b"via options").await.unwrap();
        let mut f = opts.read(true).write(true).open(&dir, "opts.txt").await.unwrap();
        let buf = f.read_max(8192).await.unwrap();
        assert_eq!(buf.len(), 11);
    }

    #[tokio::test]
    async fn from_rw_into_read_only() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("conv.txt", b"convert me").await.unwrap();
        let rw = File::open(&dir, "conv.txt").await.unwrap();
        let mut ro: ReadOnlyFile = rw.into();
        let buf = ro.read_max(8192).await.unwrap();
        assert_eq!(buf.len(), 10);
    }

    #[tokio::test]
    async fn from_rw_into_write_only() {
        let (_tmp, dir) = setup().await;
        let rw = File::create(&dir, "conv_w.txt").await.unwrap();
        let mut wo: WriteOnlyFile = rw.into();
        wo.write_slice(b"write only now").await.unwrap();
        drop(wo);
        let s = dir.read_to_string("conv_w.txt").await.unwrap();
        assert_eq!(s, "write only now");
    }

    #[tokio::test]
    async fn try_clone_works() {
        let (_tmp, dir) = setup().await;
        let f = File::create(&dir, "rwtc.txt").await.unwrap();
        let _f2 = f.try_clone().await.unwrap();
    }
}

// ===========================================================================
// OpenOptions tests
// ===========================================================================

mod open_options {
    use super::*;

    #[tokio::test]
    async fn read_open_existing() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("oo_r.txt", b"read me").await.unwrap();
        let mut f = OpenOptions::new().read(true).open(&dir, "oo_r.txt").await.unwrap();
        let buf = f.read_max(8192).await.unwrap();
        assert_eq!(buf.len(), 7);
    }

    #[tokio::test]
    async fn write_create_new_file() {
        let (_tmp, dir) = setup().await;
        let mut f = OpenOptions::new().write(true).create(true).open(&dir, "oo_wc.txt").await.unwrap();
        f.write_slice(b"new file").await.unwrap();
        drop(f);
        let s = dir.read_to_string("oo_wc.txt").await.unwrap();
        assert_eq!(s, "new file");
    }

    #[tokio::test]
    async fn create_new_fails_on_existing() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("oo_cn.txt", b"exists").await.unwrap();
        let result = OpenOptions::new().write(true).create_new(true).open(&dir, "oo_cn.txt").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn append_mode() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("oo_ap.txt", b"first").await.unwrap();
        let mut f = OpenOptions::new().append(true).open(&dir, "oo_ap.txt").await.unwrap();
        f.write_slice(b"_second").await.unwrap();
        drop(f);
        let s = dir.read_to_string("oo_ap.txt").await.unwrap();
        assert_eq!(s, "first_second");
    }

    #[tokio::test]
    async fn truncate_mode() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("oo_tr.txt", b"old content here").await.unwrap();
        let mut f = OpenOptions::new().write(true).truncate(true).open(&dir, "oo_tr.txt").await.unwrap();
        f.write_slice(b"new").await.unwrap();
        drop(f);
        let s = dir.read_to_string("oo_tr.txt").await.unwrap();
        assert_eq!(s, "new");
    }

    #[tokio::test]
    async fn open_with_memory() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("oo_mem.txt", b"memory").await.unwrap();
        let mem = GlobalPool::new();
        let mut f = OpenOptions::new()
            .read(true)
            .open_with_memory(&dir, "oo_mem.txt", mem)
            .await
            .unwrap();
        let buf = f.read_max(8192).await.unwrap();
        assert_eq!(buf.len(), 6);
    }
}

// ===========================================================================
// DirBuilder tests
// ===========================================================================

mod dir_builder {
    use std::path::Path;

    use super::*;

    #[tokio::test]
    async fn create_non_recursive() {
        let (_tmp, dir) = setup().await;
        DirBuilder::new().create(&dir, Path::new("single_dir")).await.unwrap();
        assert!(dir.exists("single_dir").await.unwrap());
    }

    #[tokio::test]
    async fn create_recursive() {
        let (_tmp, dir) = setup().await;
        DirBuilder::new().recursive(true).create(&dir, Path::new("x/y/z")).await.unwrap();
        assert!(dir.exists("x/y/z").await.unwrap());
    }

    #[tokio::test]
    async fn create_existing_non_recursive_fails() {
        let (_tmp, dir) = setup().await;
        dir.create_dir("already").await.unwrap();
        let result = DirBuilder::new().create(&dir, Path::new("already")).await;
        assert!(result.is_err());
    }
}

// ===========================================================================
// DirEntry tests
// ===========================================================================

mod dir_entry {
    use super::*;

    #[tokio::test]
    async fn file_name_returns_bare_name() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("entry.txt", b"x").await.unwrap();
        let mut rd = dir.read_dir(".").await.unwrap();
        let entry = rd.next_entry().await.unwrap().unwrap();
        assert_eq!(entry.file_name(), OsString::from("entry.txt"));
    }

    #[tokio::test]
    async fn metadata_returns_correct_info() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("info.txt", b"12345").await.unwrap();
        let mut rd = dir.read_dir(".").await.unwrap();
        let entry = rd.next_entry().await.unwrap().unwrap();
        let md = entry.metadata().unwrap();
        assert!(md.is_file());
        assert_eq!(md.len(), 5);
    }

    #[tokio::test]
    async fn file_type_for_file() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("ft.txt", b"x").await.unwrap();
        let mut rd = dir.read_dir(".").await.unwrap();
        let entry = rd.next_entry().await.unwrap().unwrap();
        let ft = entry.file_type().unwrap();
        assert!(ft.is_file());
    }

    #[tokio::test]
    async fn file_type_for_dir() {
        let (_tmp, dir) = setup().await;
        dir.create_dir("sub").await.unwrap();
        let mut rd = dir.read_dir(".").await.unwrap();
        let entry = rd.next_entry().await.unwrap().unwrap();
        let ft = entry.file_type().unwrap();
        assert!(ft.is_dir());
    }
}

// ===========================================================================
// Edge case tests
// ===========================================================================

mod edge_cases {
    use super::*;

    #[tokio::test]
    async fn empty_file_read_returns_zero_bytes() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("empty.txt", b"").await.unwrap();
        let mut f = ReadOnlyFile::open(&dir, "empty.txt").await.unwrap();
        let buf = f.read_max(8192).await.unwrap();
        assert_eq!(buf.len(), 0);
    }

    #[tokio::test]
    async fn large_write_and_read_1mb() {
        let (_tmp, dir) = setup().await;
        let size = 1024 * 1024; // 1 MB
        let data = vec![0xABu8; size];
        dir.write_slice("large.bin", &data).await.unwrap();
        let view = dir.read("large.bin").await.unwrap();
        let mut collected = Vec::new();
        let mut v = view;
        while !v.is_empty() {
            let s = v.first_slice();
            collected.extend_from_slice(s);
            let len = s.len();
            v.advance(len);
        }
        assert_eq!(collected.len(), size);
        assert!(collected.iter().all(|&b| b == 0xAB));
    }

    #[tokio::test]
    async fn unicode_filenames() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("日本語ファイル.txt", b"unicode").await.unwrap();
        let s = dir.read_to_string("日本語ファイル.txt").await.unwrap();
        assert_eq!(s, "unicode");
    }

    #[tokio::test]
    async fn deeply_nested_directories() {
        let (_tmp, dir) = setup().await;
        let deep = "a/b/c/d/e/f/g/h/i/j";
        dir.create_dir_all(deep).await.unwrap();
        dir.write_slice(&format!("{deep}/deep.txt"), b"deep").await.unwrap();
        let s = dir.read_to_string(&format!("{deep}/deep.txt")).await.unwrap();
        assert_eq!(s, "deep");
    }

    #[tokio::test]
    async fn concurrent_read_at_positional_io() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("conc.txt", b"ABCDEFGHIJKLMNOPQRSTUVWXYZ").await.unwrap();
        let f = ReadOnlyPositionalFile::open(&dir, "conc.txt").await.unwrap();

        let f1 = f.try_clone().await.unwrap();
        let f2 = f.try_clone().await.unwrap();
        let f3 = f.try_clone().await.unwrap();

        let (r1, r2, r3) = tokio::join!(f1.read_at(0, 5), f2.read_at(10, 5), f3.read_at(20, 5),);

        let collect = |view: bytesbuf::BytesView| -> Vec<u8> {
            let mut out = Vec::new();
            let mut v = view;
            while !v.is_empty() {
                let s = v.first_slice();
                out.extend_from_slice(s);
                let len = s.len();
                v.advance(len);
            }
            out
        };

        assert_eq!(collect(r1.unwrap()), b"ABCDE");
        assert_eq!(collect(r2.unwrap()), b"KLMNO");
        assert_eq!(collect(r3.unwrap()), b"UVWXY");
    }

    #[tokio::test]
    async fn emoji_filename() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("🚀🎉.txt", b"rocket party").await.unwrap();
        let s = dir.read_to_string("🚀🎉.txt").await.unwrap();
        assert_eq!(s, "rocket party");
    }

    #[tokio::test]
    async fn write_empty_file() {
        let (_tmp, dir) = setup().await;
        let mut f = WriteOnlyFile::create(&dir, "empty_w.txt").await.unwrap();
        f.write_slice(b"").await.unwrap();
        drop(f);
        let md = dir.metadata("empty_w.txt").await.unwrap();
        assert_eq!(md.len(), 0);
    }

    #[tokio::test]
    async fn path_with_interior_dotdot_within_root() {
        let (_tmp, dir) = setup().await;
        dir.create_dir("a").await.unwrap();
        dir.create_dir("b").await.unwrap();
        dir.write_slice("b/f.txt", b"found").await.unwrap();
        // "a/../b/f.txt" should resolve within root
        let result = dir.read_to_string("a/../b/f.txt").await;
        // It should either succeed (path resolves within root) or fail
        // gracefully (path traversal rejected). Both are valid.
        if let Ok(s) = result {
            assert_eq!(s, "found");
        }
    }

    #[tokio::test]
    async fn read_at_beyond_eof() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("small.txt", b"hi").await.unwrap();
        let f = ReadOnlyPositionalFile::open(&dir, "small.txt").await.unwrap();
        let view = f.read_at(100, 10).await.unwrap();
        assert_eq!(view.len(), 0);
    }

    #[tokio::test]
    async fn write_at_beyond_eof_extends_file() {
        let (_tmp, dir) = setup().await;
        let f = WriteOnlyPositionalFile::create(&dir, "extend.txt").await.unwrap();
        f.write_slice_at(0, b"AB").await.unwrap();
        f.write_slice_at(10, b"CD").await.unwrap();
        let md = f.metadata().await.unwrap();
        assert_eq!(md.len(), 12);
    }

    #[tokio::test]
    async fn multiple_sequential_writes() {
        let (_tmp, dir) = setup().await;
        let mut f = WriteOnlyFile::create(&dir, "multi.txt").await.unwrap();
        for i in 0..100 {
            let line = format!("line {i}\n");
            f.write_slice(line.as_bytes()).await.unwrap();
        }
        drop(f);
        let s = dir.read_to_string("multi.txt").await.unwrap();
        assert!(s.contains("line 0\n"));
        assert!(s.contains("line 99\n"));
    }

    #[tokio::test]
    async fn seek_from_end() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("sfe.txt", b"0123456789").await.unwrap();
        let mut f = ReadOnlyFile::open(&dir, "sfe.txt").await.unwrap();
        let pos = f.seek(SeekFrom::End(-3)).await.unwrap();
        assert_eq!(pos, 7);
        let mut buf = [0u8; 3];
        let n = f.read_max_into_slice(3, &mut buf).await.unwrap();
        assert_eq!(n, 3);
        assert_eq!(&buf, b"789");
    }

    #[tokio::test]
    async fn seek_from_current() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("sfc.txt", b"ABCDEFGHIJ").await.unwrap();
        let mut f = ReadOnlyFile::open(&dir, "sfc.txt").await.unwrap();
        f.seek(SeekFrom::Start(2)).await.unwrap();
        let pos = f.seek(SeekFrom::Current(3)).await.unwrap();
        assert_eq!(pos, 5);
    }

    #[tokio::test]
    async fn read_dir_with_mixed_entries() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("file1.txt", b"a").await.unwrap();
        dir.create_dir("subdir").await.unwrap();
        dir.write_slice("file2.txt", b"b").await.unwrap();
        let mut rd = dir.read_dir(".").await.unwrap();
        let mut count = 0;
        while rd.next_entry().await.unwrap().is_some() {
            count += 1;
        }
        assert_eq!(count, 3);
    }
}

// ===========================================================================
// New read API tests (ReadOnlyFile)
// ===========================================================================

mod read_only_new_api {
    use core::mem::MaybeUninit;

    use super::*;

    #[tokio::test]
    async fn read_best_effort_fills_fully() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("rbf.txt", b"0123456789").await.unwrap();
        let mut f = ReadOnlyFile::open(&dir, "rbf.txt").await.unwrap();
        let view = f.read(10).await.unwrap();
        assert_eq!(view.len(), 10);
    }

    #[tokio::test]
    async fn read_best_effort_partial_at_eof() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("rbp.txt", b"short").await.unwrap();
        let mut f = ReadOnlyFile::open(&dir, "rbp.txt").await.unwrap();
        let view = f.read(100).await.unwrap();
        assert_eq!(view.len(), 5);
    }

    #[tokio::test]
    async fn read_exact_success() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("re.txt", b"exact_data!").await.unwrap();
        let mut f = ReadOnlyFile::open(&dir, "re.txt").await.unwrap();
        let view = f.read_exact(5).await.unwrap();
        assert_eq!(view.len(), 5);
    }

    #[tokio::test]
    async fn read_exact_eof_is_error() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("ree.txt", b"hi").await.unwrap();
        let mut f = ReadOnlyFile::open(&dir, "ree.txt").await.unwrap();
        let err = f.read_exact(100).await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn read_max_at_single_operation() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("rma.txt", b"0123456789").await.unwrap();
        let f = ReadOnlyPositionalFile::open(&dir, "rma.txt").await.unwrap();
        let view = f.read_max_at(3, 4).await.unwrap();
        assert!(view.len() <= 4);
        assert!(!view.is_empty());
    }

    #[tokio::test]
    async fn read_into_bytebuf_at_works() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("riba.txt", b"ABCDEFGHIJ").await.unwrap();
        let f = ReadOnlyPositionalFile::open(&dir, "riba.txt").await.unwrap();
        let mem = GlobalPool::new();
        let mut buf = mem.reserve(16);
        let n = f.read_into_bytebuf_at(5, &mut buf).await.unwrap();
        assert!(n > 0);
    }

    #[tokio::test]
    async fn read_exact_into_bytebuf_success() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("reib.txt", b"hello world").await.unwrap();
        let mut f = ReadOnlyFile::open(&dir, "reib.txt").await.unwrap();
        let mem = GlobalPool::new();
        let mut buf = mem.reserve(32);
        f.read_exact_into_bytebuf(5, &mut buf).await.unwrap();
        assert_eq!(buf.len(), 5);
    }

    #[tokio::test]
    async fn read_exact_into_bytebuf_eof_is_error() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("reibf.txt", b"hi").await.unwrap();
        let mut f = ReadOnlyFile::open(&dir, "reibf.txt").await.unwrap();
        let mem = GlobalPool::new();
        let mut buf = mem.reserve(32);
        let err = f.read_exact_into_bytebuf(100, &mut buf).await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn read_exact_into_bytebuf_at_success() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("reiba.txt", b"0123456789").await.unwrap();
        let f = ReadOnlyPositionalFile::open(&dir, "reiba.txt").await.unwrap();
        let mem = GlobalPool::new();
        let mut buf = mem.reserve(32);
        f.read_exact_into_bytebuf_at(2, 4, &mut buf).await.unwrap();
        assert_eq!(buf.len(), 4);
    }

    #[tokio::test]
    async fn read_exact_into_bytebuf_at_eof_is_error() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("reibaf.txt", b"hi").await.unwrap();
        let f = ReadOnlyPositionalFile::open(&dir, "reibaf.txt").await.unwrap();
        let mem = GlobalPool::new();
        let mut buf = mem.reserve(32);
        let err = f.read_exact_into_bytebuf_at(0, 100, &mut buf).await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn read_into_slice_fills_fully() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("ris.txt", b"0123456789").await.unwrap();
        let mut f = ReadOnlyFile::open(&dir, "ris.txt").await.unwrap();
        let mut buf = [0u8; 10];
        let n = f.read_into_slice(&mut buf).await.unwrap();
        assert_eq!(n, 10);
        assert_eq!(&buf, b"0123456789");
    }

    #[tokio::test]
    async fn read_exact_into_slice_success() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("reis.txt", b"hello").await.unwrap();
        let mut f = ReadOnlyFile::open(&dir, "reis.txt").await.unwrap();
        let mut buf = [0u8; 5];
        f.read_exact_into_slice(&mut buf).await.unwrap();
        assert_eq!(&buf, b"hello");
    }

    #[tokio::test]
    async fn read_exact_into_slice_eof_is_error() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("reisf.txt", b"hi").await.unwrap();
        let mut f = ReadOnlyFile::open(&dir, "reisf.txt").await.unwrap();
        let mut buf = [0u8; 100];
        let err = f.read_exact_into_slice(&mut buf).await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn read_into_slice_at_fills() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("risa.txt", b"ABCDEFGHIJ").await.unwrap();
        let f = ReadOnlyPositionalFile::open(&dir, "risa.txt").await.unwrap();
        let mut buf = [0u8; 5];
        let n = f.read_into_slice_at(5, &mut buf).await.unwrap();
        assert_eq!(n, 5);
        assert_eq!(&buf, b"FGHIJ");
    }

    #[tokio::test]
    async fn read_exact_into_slice_at_success() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("reisa.txt", b"ABCDEFGHIJ").await.unwrap();
        let f = ReadOnlyPositionalFile::open(&dir, "reisa.txt").await.unwrap();
        let mut buf = [0u8; 3];
        f.read_exact_into_slice_at(7, &mut buf).await.unwrap();
        assert_eq!(&buf, b"HIJ");
    }

    #[tokio::test]
    async fn read_exact_into_slice_at_eof_is_error() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("reisaf.txt", b"hi").await.unwrap();
        let f = ReadOnlyPositionalFile::open(&dir, "reisaf.txt").await.unwrap();
        let mut buf = [0u8; 100];
        let err = f.read_exact_into_slice_at(0, &mut buf).await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn read_exact_into_uninit_success() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("reiu.txt", b"uninit_test").await.unwrap();
        let mut f = ReadOnlyFile::open(&dir, "reiu.txt").await.unwrap();
        let mut buf = [MaybeUninit::<u8>::uninit(); 6];
        f.read_exact_into_uninit(&mut buf).await.unwrap();
        // SAFETY: read_exact_into_uninit guarantees initialization on success.
        let initialized = unsafe { core::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), buf.len()) };
        assert_eq!(initialized, b"uninit");
    }

    #[tokio::test]
    async fn read_exact_into_uninit_at_success() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("reiua.txt", b"0123456789").await.unwrap();
        let f = ReadOnlyPositionalFile::open(&dir, "reiua.txt").await.unwrap();
        let mut buf = [MaybeUninit::<u8>::uninit(); 3];
        f.read_exact_into_uninit_at(7, &mut buf).await.unwrap();
        // SAFETY: read_exact_into_uninit_at guarantees initialization on success.
        let initialized = unsafe { core::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), buf.len()) };
        assert_eq!(initialized, b"789");
    }
}

// ===========================================================================
// Sync Read / Write / Seek trait tests
// ===========================================================================

#[cfg(feature = "sync-compat")]
mod sync_io_traits {
    use std::io::{BufRead, BufReader, Read, Seek, Write};

    use super::*;

    #[tokio::test]
    async fn read_only_file_sync_read() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("sr.txt", b"sync read test").await.unwrap();
        let mut f = ReadOnlyFile::open(&dir, "sr.txt").await.unwrap();
        let mut buf = [0u8; 9];
        let n = Read::read(&mut f, &mut buf).unwrap();
        assert_eq!(n, 9);
        assert_eq!(&buf, b"sync read");
    }

    #[tokio::test]
    async fn read_only_file_sync_seek() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("ss.txt", b"0123456789").await.unwrap();
        let mut f = ReadOnlyFile::open(&dir, "ss.txt").await.unwrap();
        let pos = Seek::seek(&mut f, SeekFrom::Start(5)).unwrap();
        assert_eq!(pos, 5);
        let mut buf = [0u8; 5];
        let n = Read::read(&mut f, &mut buf).unwrap();
        assert_eq!(n, 5);
        assert_eq!(&buf, b"56789");
    }

    #[tokio::test]
    async fn read_only_file_with_bufreader() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("br.txt", b"line1\nline2\nline3\n").await.unwrap();
        let f = ReadOnlyFile::open(&dir, "br.txt").await.unwrap();
        let reader = BufReader::new(f);
        let lines: Vec<String> = reader.lines().map(|l| l.unwrap()).collect();
        assert_eq!(lines, vec!["line1", "line2", "line3"]);
    }

    #[tokio::test]
    async fn write_only_file_sync_write() {
        let (_tmp, dir) = setup().await;
        let mut f = WriteOnlyFile::create(&dir, "sw.txt").await.unwrap();
        let n = Write::write(&mut f, b"sync write").unwrap();
        assert_eq!(n, 10);
        Write::flush(&mut f).unwrap();
        drop(f);
        let s = dir.read_to_string("sw.txt").await.unwrap();
        assert_eq!(s, "sync write");
    }

    #[tokio::test]
    async fn write_only_file_sync_seek() {
        let (_tmp, dir) = setup().await;
        let mut f = WriteOnlyFile::create(&dir, "sws.txt").await.unwrap();
        Write::write_all(&mut f, b"AAAAAAAAAA").unwrap();
        Seek::seek(&mut f, SeekFrom::Start(3)).unwrap();
        Write::write_all(&mut f, b"BB").unwrap();
        Write::flush(&mut f).unwrap();
        drop(f);
        let s = dir.read_to_string("sws.txt").await.unwrap();
        assert_eq!(s, "AAABBAAAAA");
    }

    #[tokio::test]
    async fn read_write_file_sync_read_write() {
        let (_tmp, dir) = setup().await;
        let mut f = File::create(&dir, "srw.txt").await.unwrap();
        Write::write_all(&mut f, b"hello world").unwrap();
        Seek::seek(&mut f, SeekFrom::Start(0)).unwrap();
        let mut buf = [0u8; 5];
        Read::read_exact(&mut f, &mut buf).unwrap();
        assert_eq!(&buf, b"hello");
    }

    #[tokio::test]
    async fn read_write_file_sync_seek_stream_position() {
        let (_tmp, dir) = setup().await;
        let mut f = File::create(&dir, "ssp.txt").await.unwrap();
        Write::write_all(&mut f, b"0123456789").unwrap();
        let pos = Seek::stream_position(&mut f).unwrap();
        assert_eq!(pos, 10);
        Seek::rewind(&mut f).unwrap();
        let pos = Seek::stream_position(&mut f).unwrap();
        assert_eq!(pos, 0);
    }
}

// ===========================================================================
// Platform fd/handle trait tests
// ===========================================================================

mod platform_traits {
    use super::*;

    #[tokio::test]
    async fn read_only_file_as_raw_fd_or_handle() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("fd.txt", b"x").await.unwrap();
        let f = ReadOnlyFile::open(&dir, "fd.txt").await.unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::io::{AsFd, AsRawFd};
            let raw = f.as_raw_fd();
            assert!(raw >= 0);
            let borrowed = f.as_fd();
            assert_eq!(std::os::unix::io::AsRawFd::as_raw_fd(&borrowed), raw);
        }

        #[cfg(windows)]
        {
            use std::os::windows::io::{AsHandle, AsRawHandle};
            let raw = f.as_raw_handle();
            assert!(!raw.is_null());
            let _borrowed = f.as_handle();
        }
    }

    #[tokio::test]
    async fn write_only_file_as_raw_fd_or_handle() {
        let (_tmp, dir) = setup().await;
        let f = WriteOnlyFile::create(&dir, "wfd.txt").await.unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::io::{AsFd, AsRawFd};
            let raw = f.as_raw_fd();
            assert!(raw >= 0);
            let _borrowed = f.as_fd();
        }

        #[cfg(windows)]
        {
            use std::os::windows::io::{AsHandle, AsRawHandle};
            let raw = f.as_raw_handle();
            assert!(!raw.is_null());
            let _borrowed = f.as_handle();
        }
    }

    #[tokio::test]
    async fn read_write_file_as_raw_fd_or_handle() {
        let (_tmp, dir) = setup().await;
        let f = File::create(&dir, "rwfd.txt").await.unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::io::{AsFd, AsRawFd};
            let raw = f.as_raw_fd();
            assert!(raw >= 0);
            let _borrowed = f.as_fd();
        }

        #[cfg(windows)]
        {
            use std::os::windows::io::{AsHandle, AsRawHandle};
            let raw = f.as_raw_handle();
            assert!(!raw.is_null());
            let _borrowed = f.as_handle();
        }
    }
}

// ===========================================================================
// File new read API tests
// ===========================================================================

mod read_write_new_api {
    use super::*;

    #[tokio::test]
    async fn read_exact_success() {
        let (_tmp, dir) = setup().await;
        let mut f = File::create(&dir, "rwre.txt").await.unwrap();
        f.write_slice(b"exact_data!").await.unwrap();
        f.rewind().await.unwrap();
        let view = f.read_exact(5).await.unwrap();
        assert_eq!(view.len(), 5);
    }

    #[tokio::test]
    async fn read_exact_eof_is_error() {
        let (_tmp, dir) = setup().await;
        let mut f = File::create(&dir, "rwref.txt").await.unwrap();
        f.write_slice(b"hi").await.unwrap();
        f.rewind().await.unwrap();
        let err = f.read_exact(100).await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn read_exact_into_uninit() {
        let (_tmp, dir) = setup().await;
        let mut f = File::create(&dir, "rwreiu.txt").await.unwrap();
        f.write_slice(b"uninit_test").await.unwrap();
        f.rewind().await.unwrap();
        let mut buf = [core::mem::MaybeUninit::<u8>::uninit(); 6];
        f.read_exact_into_uninit(&mut buf).await.unwrap();
        // SAFETY: read_exact_into_uninit guarantees initialization on success.
        let initialized = unsafe { core::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), buf.len()) };
        assert_eq!(initialized, b"uninit");
    }
}

// ===========================================================================
// ReadOnlyPositionalFile tests
// ===========================================================================

mod read_only_positional_file {
    use core::mem::MaybeUninit;

    use super::*;

    #[tokio::test]
    async fn open_and_read_at() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("rat.txt", b"0123456789").await.unwrap();
        let f = ReadOnlyPositionalFile::open(&dir, "rat.txt").await.unwrap();
        let view = f.read_at(5, 5).await.unwrap();
        let mut collected = Vec::new();
        let mut v = view;
        while !v.is_empty() {
            let s = v.first_slice();
            collected.extend_from_slice(s);
            let len = s.len();
            v.advance(len);
        }
        assert_eq!(collected, b"56789");
    }

    #[tokio::test]
    async fn read_max_into_bytebuf_at_works() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("rai.txt", b"ABCDEFGHIJ").await.unwrap();
        let f = ReadOnlyPositionalFile::open(&dir, "rai.txt").await.unwrap();
        let mem = GlobalPool::new();
        let mut buf = mem.reserve(16);
        let n = f.read_max_into_bytebuf_at(2, 4, &mut buf).await.unwrap();
        assert!(n > 0);
        assert!(!buf.is_empty());
    }

    #[tokio::test]
    async fn read_exact_at_success() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("rea.txt", b"exact read test!").await.unwrap();
        let f = ReadOnlyPositionalFile::open(&dir, "rea.txt").await.unwrap();
        let view = f.read_exact_at(6, 4).await.unwrap();
        let mut collected = Vec::new();
        let mut v = view;
        while !v.is_empty() {
            let s = v.first_slice();
            collected.extend_from_slice(s);
            let len = s.len();
            v.advance(len);
        }
        assert_eq!(collected, b"read");
    }

    #[tokio::test]
    async fn read_exact_at_unexpected_eof() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("short.txt", b"hi").await.unwrap();
        let f = ReadOnlyPositionalFile::open(&dir, "short.txt").await.unwrap();
        let result = f.read_exact_at(0, 100).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn read_max_into_slice_at_works() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("rs.txt", b"slice_test").await.unwrap();
        let f = ReadOnlyPositionalFile::open(&dir, "rs.txt").await.unwrap();
        let mut buf = [0u8; 4];
        let n = f.read_max_into_slice_at(6, 4, &mut buf).await.unwrap();
        assert_eq!(n, 4);
        assert_eq!(&buf[..n], b"test");
    }

    #[tokio::test]
    async fn open_with_memory() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("owm.txt", b"with memory").await.unwrap();
        let mem = GlobalPool::new();
        let f = ReadOnlyPositionalFile::open_with_memory(&dir, "owm.txt", mem).await.unwrap();
        let view = f.read_at(0, 11).await.unwrap();
        assert_eq!(view.len(), 11);
    }

    #[tokio::test]
    async fn metadata_works() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("md.txt", b"12345678").await.unwrap();
        let f = ReadOnlyPositionalFile::open(&dir, "md.txt").await.unwrap();
        let md = f.metadata().await.unwrap();
        assert_eq!(md.len(), 8);
        assert!(md.is_file());
    }

    #[tokio::test]
    async fn try_clone_works() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("clone.txt", b"clone data").await.unwrap();
        let f = ReadOnlyPositionalFile::open(&dir, "clone.txt").await.unwrap();
        let _f2 = f.try_clone().await.unwrap();
        let md = f.metadata().await.unwrap();
        assert_eq!(md.len(), 10);
    }

    #[tokio::test]
    async fn lock_and_unlock() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("lock.txt", b"x").await.unwrap();
        let f = ReadOnlyPositionalFile::open(&dir, "lock.txt").await.unwrap();
        f.lock().await.unwrap();
        f.unlock().await.unwrap();
    }

    #[tokio::test]
    async fn read_exact_into_uninit_at_success() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("reiua.txt", b"0123456789").await.unwrap();
        let f = ReadOnlyPositionalFile::open(&dir, "reiua.txt").await.unwrap();
        let mut buf = [MaybeUninit::<u8>::uninit(); 3];
        f.read_exact_into_uninit_at(7, &mut buf).await.unwrap();
        // SAFETY: read_exact_into_uninit_at guarantees initialization on success.
        let initialized = unsafe { core::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), buf.len()) };
        assert_eq!(initialized, b"789");
    }
}

// ===========================================================================
// WriteOnlyPositionalFile tests
// ===========================================================================

mod write_only_positional_file {
    use super::*;

    #[tokio::test]
    async fn write_at_positional() {
        let (_tmp, dir) = setup().await;
        let f = WriteOnlyPositionalFile::create(&dir, "wat.txt").await.unwrap();
        f.write_slice_at(0, b"AAAAAAAAAA").await.unwrap();
        let data = make_view(b"BB");
        f.write_at(3, data).await.unwrap();
        drop(f);
        let s = dir.read_to_string("wat.txt").await.unwrap();
        assert_eq!(s, "AAABBAAAAA");
    }

    #[tokio::test]
    async fn write_at_positional_all() {
        let (_tmp, dir) = setup().await;
        let f = WriteOnlyPositionalFile::create(&dir, "waat.txt").await.unwrap();
        f.write_slice_at(0, b"XXXXXXXXXX").await.unwrap();
        let data = make_view(b"YYY");
        f.write_at(7, data).await.unwrap();
        drop(f);
        let s = dir.read_to_string("waat.txt").await.unwrap();
        assert_eq!(s, "XXXXXXXYYY");
    }

    #[tokio::test]
    async fn write_slice_at_positional() {
        let (_tmp, dir) = setup().await;
        let f = WriteOnlyPositionalFile::create(&dir, "wsa.txt").await.unwrap();
        f.write_slice_at(0, b"0000000000").await.unwrap();
        f.write_slice_at(2, b"11").await.unwrap();
        drop(f);
        let s = dir.read_to_string("wsa.txt").await.unwrap();
        assert_eq!(s, "0011000000");
    }

    #[tokio::test]
    async fn create_new_succeeds_then_fails() {
        let (_tmp, dir) = setup().await;
        let f = WriteOnlyPositionalFile::create_new(&dir, "new.txt").await.unwrap();
        drop(f);
        let result = WriteOnlyPositionalFile::create_new(&dir, "new.txt").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn create_with_memory_works() {
        let (_tmp, dir) = setup().await;
        let mem = GlobalPool::new();
        let f = WriteOnlyPositionalFile::create_with_memory(&dir, "cwm.txt", mem).await.unwrap();
        f.write_slice_at(0, b"with memory").await.unwrap();
        drop(f);
        let s = dir.read_to_string("cwm.txt").await.unwrap();
        assert_eq!(s, "with memory");
    }

    #[tokio::test]
    async fn create_new_with_memory_works() {
        let (_tmp, dir) = setup().await;
        let mem = GlobalPool::new();
        let f = WriteOnlyPositionalFile::create_new_with_memory(&dir, "cnwm.txt", mem)
            .await
            .unwrap();
        f.write_slice_at(0, b"new with mem").await.unwrap();
        drop(f);
        let s = dir.read_to_string("cnwm.txt").await.unwrap();
        assert_eq!(s, "new with mem");
    }

    #[tokio::test]
    async fn set_len_truncate() {
        let (_tmp, dir) = setup().await;
        let f = WriteOnlyPositionalFile::create(&dir, "slt.txt").await.unwrap();
        f.write_slice_at(0, b"1234567890").await.unwrap();
        f.set_len(5).await.unwrap();
        drop(f);
        let s = dir.read_to_string("slt.txt").await.unwrap();
        assert_eq!(s, "12345");
    }

    #[tokio::test]
    async fn metadata_works() {
        let (_tmp, dir) = setup().await;
        let f = WriteOnlyPositionalFile::create(&dir, "wmd.txt").await.unwrap();
        f.write_slice_at(0, b"12345").await.unwrap();
        let md = f.metadata().await.unwrap();
        assert_eq!(md.len(), 5);
    }

    #[tokio::test]
    async fn flush_sync_all_sync_data() {
        let (_tmp, dir) = setup().await;
        let f = WriteOnlyPositionalFile::create(&dir, "sync.txt").await.unwrap();
        f.write_slice_at(0, b"sync data").await.unwrap();
        f.flush().await.unwrap();
        f.sync_all().await.unwrap();
        f.sync_data().await.unwrap();
    }

    #[tokio::test]
    async fn try_clone_works() {
        let (_tmp, dir) = setup().await;
        let f = WriteOnlyPositionalFile::create(&dir, "tc.txt").await.unwrap();
        let _f2 = f.try_clone().await.unwrap();
    }

    #[tokio::test]
    async fn lock_and_unlock() {
        let (_tmp, dir) = setup().await;
        let f = WriteOnlyPositionalFile::create(&dir, "lock.txt").await.unwrap();
        f.lock().await.unwrap();
        f.unlock().await.unwrap();
    }
}

// ===========================================================================
// PositionalFile tests
// ===========================================================================

mod positional_file {
    use super::*;

    #[tokio::test]
    async fn open_existing() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("pf.txt", b"existing").await.unwrap();
        let _f = PositionalFile::open(&dir, "pf.txt").await.unwrap();
    }

    #[tokio::test]
    async fn create_new_file() {
        let (_tmp, dir) = setup().await;
        let f = PositionalFile::create(&dir, "pf_new.txt").await.unwrap();
        f.write_slice_at(0, b"created").await.unwrap();
        drop(f);
        let s = dir.read_to_string("pf_new.txt").await.unwrap();
        assert_eq!(s, "created");
    }

    #[tokio::test]
    async fn create_truncates_existing() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("trunc.txt", b"old data old data").await.unwrap();
        let f = PositionalFile::create(&dir, "trunc.txt").await.unwrap();
        f.write_slice_at(0, b"new").await.unwrap();
        drop(f);
        let s = dir.read_to_string("trunc.txt").await.unwrap();
        assert_eq!(s, "new");
    }

    #[tokio::test]
    async fn create_new_fails_on_existing() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("exists.txt", b"x").await.unwrap();
        let result = PositionalFile::create_new(&dir, "exists.txt").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn create_new_succeeds_on_new() {
        let (_tmp, dir) = setup().await;
        let _f = PositionalFile::create_new(&dir, "brand_new.txt").await.unwrap();
    }

    #[tokio::test]
    async fn open_with_memory_works() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("owm.txt", b"data").await.unwrap();
        let mem = GlobalPool::new();
        let _f = PositionalFile::open_with_memory(&dir, "owm.txt", mem).await.unwrap();
    }

    #[tokio::test]
    async fn create_with_memory_works() {
        let (_tmp, dir) = setup().await;
        let mem = GlobalPool::new();
        let f = PositionalFile::create_with_memory(&dir, "cwm.txt", mem).await.unwrap();
        f.write_slice_at(0, b"mem data").await.unwrap();
        drop(f);
        let s = dir.read_to_string("cwm.txt").await.unwrap();
        assert_eq!(s, "mem data");
    }

    #[tokio::test]
    async fn create_new_with_memory_works() {
        let (_tmp, dir) = setup().await;
        let mem = GlobalPool::new();
        let f = PositionalFile::create_new_with_memory(&dir, "cnwm.txt", mem).await.unwrap();
        f.write_slice_at(0, b"new mem").await.unwrap();
        drop(f);
        let s = dir.read_to_string("cnwm.txt").await.unwrap();
        assert_eq!(s, "new mem");
    }

    #[tokio::test]
    async fn read_at_and_write_at_interleaved() {
        let (_tmp, dir) = setup().await;
        let f = PositionalFile::create(&dir, "interl.txt").await.unwrap();
        f.write_slice_at(0, b"ABCDEFGHIJ").await.unwrap();

        let view = f.read_at(0, 3).await.unwrap();
        let mut collected = Vec::new();
        let mut v = view;
        while !v.is_empty() {
            let s = v.first_slice();
            collected.extend_from_slice(s);
            let len = s.len();
            v.advance(len);
        }
        assert_eq!(collected, b"ABC");

        let data = make_view(b"XY");
        f.write_at(3, data).await.unwrap();

        let view2 = f.read_at(3, 2).await.unwrap();
        let mut collected2 = Vec::new();
        let mut v2 = view2;
        while !v2.is_empty() {
            let s = v2.first_slice();
            collected2.extend_from_slice(s);
            let len = s.len();
            v2.advance(len);
        }
        assert_eq!(collected2, b"XY");
    }

    #[tokio::test]
    async fn read_exact_at_works() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("rea_pf.txt", b"exactpfdata").await.unwrap();
        let f = PositionalFile::open(&dir, "rea_pf.txt").await.unwrap();
        let view = f.read_exact_at(5, 2).await.unwrap();
        let mut collected = Vec::new();
        let mut v = view;
        while !v.is_empty() {
            let s = v.first_slice();
            collected.extend_from_slice(s);
            let len = s.len();
            v.advance(len);
        }
        assert_eq!(collected, b"pf");
    }

    #[tokio::test]
    async fn read_slice_at_and_write_slice_at() {
        let (_tmp, dir) = setup().await;
        let f = PositionalFile::create(&dir, "rsa.txt").await.unwrap();
        f.write_slice_at(0, b"0123456789").await.unwrap();
        f.write_slice_at(2, b"AB").await.unwrap();
        let mut buf = [0u8; 4];
        let n = f.read_max_into_slice_at(1, 4, &mut buf).await.unwrap();
        assert_eq!(n, 4);
        assert_eq!(&buf[..n], b"1AB4");
    }

    #[tokio::test]
    async fn metadata_and_set_len() {
        let (_tmp, dir) = setup().await;
        let f = PositionalFile::create(&dir, "pfmd.txt").await.unwrap();
        f.write_slice_at(0, b"12345").await.unwrap();
        let md = f.metadata().await.unwrap();
        assert_eq!(md.len(), 5);
        f.set_len(3).await.unwrap();
        let md2 = f.metadata().await.unwrap();
        assert_eq!(md2.len(), 3);
    }

    #[tokio::test]
    async fn lock_unlock_cycle() {
        let (_tmp, dir) = setup().await;
        let f = PositionalFile::create(&dir, "pflock.txt").await.unwrap();
        f.lock().await.unwrap();
        f.unlock().await.unwrap();
        f.lock_shared().await.unwrap();
        f.unlock().await.unwrap();
        f.try_lock().await.unwrap();
        f.unlock().await.unwrap();
        f.try_lock_shared().await.unwrap();
        f.unlock().await.unwrap();
    }

    #[tokio::test]
    async fn flush_sync_all_sync_data() {
        let (_tmp, dir) = setup().await;
        let f = PositionalFile::create(&dir, "pfsync.txt").await.unwrap();
        f.write_slice_at(0, b"sync").await.unwrap();
        f.flush().await.unwrap();
        f.sync_all().await.unwrap();
        f.sync_data().await.unwrap();
    }

    #[tokio::test]
    async fn set_permissions_and_set_modified_and_set_times() {
        let (_tmp, dir) = setup().await;
        let f = PositionalFile::create(&dir, "pfperms.txt").await.unwrap();
        let md = f.metadata().await.unwrap();
        f.set_permissions(md.permissions()).await.unwrap();
        let t = SystemTime::now() - Duration::from_secs(100);
        f.set_modified(t).await.unwrap();
        let times = file::FileTimes::new().set_modified(SystemTime::now());
        f.set_times(times).await.unwrap();
    }

    #[tokio::test]
    async fn from_positional_into_read_only_positional() {
        let (_tmp, dir) = setup().await;
        dir.write_slice("conv.txt", b"convert me").await.unwrap();
        let pf = PositionalFile::open(&dir, "conv.txt").await.unwrap();
        let ro: ReadOnlyPositionalFile = pf.into();
        let view = ro.read_at(0, 10).await.unwrap();
        assert_eq!(view.len(), 10);
    }

    #[tokio::test]
    async fn from_positional_into_write_only_positional() {
        let (_tmp, dir) = setup().await;
        let pf = PositionalFile::create(&dir, "conv_w.txt").await.unwrap();
        let wo: WriteOnlyPositionalFile = pf.into();
        wo.write_slice_at(0, b"write only now").await.unwrap();
        drop(wo);
        let s = dir.read_to_string("conv_w.txt").await.unwrap();
        assert_eq!(s, "write only now");
    }

    #[tokio::test]
    async fn try_clone_works() {
        let (_tmp, dir) = setup().await;
        let f = PositionalFile::create(&dir, "pftc.txt").await.unwrap();
        let _f2 = f.try_clone().await.unwrap();
    }

    #[tokio::test]
    async fn read_max_at_single_operation() {
        let (_tmp, dir) = setup().await;
        let f = PositionalFile::create(&dir, "pfrma.txt").await.unwrap();
        f.write_slice_at(0, b"0123456789").await.unwrap();
        let view = f.read_max_at(3, 4).await.unwrap();
        assert!(view.len() <= 4);
        assert!(!view.is_empty());
    }

    #[tokio::test]
    async fn read_into_bytebuf_at_works() {
        let (_tmp, dir) = setup().await;
        let f = PositionalFile::create(&dir, "pfriba.txt").await.unwrap();
        f.write_slice_at(0, b"ABCDEFGHIJ").await.unwrap();
        let mem = GlobalPool::new();
        let mut buf = mem.reserve(16);
        let n = f.read_into_bytebuf_at(5, &mut buf).await.unwrap();
        assert!(n > 0);
    }

    #[tokio::test]
    async fn read_exact_into_uninit_at() {
        let (_tmp, dir) = setup().await;
        let f = PositionalFile::create(&dir, "pfreiu.txt").await.unwrap();
        f.write_slice_at(0, b"uninit_test").await.unwrap();
        let mut buf = [core::mem::MaybeUninit::<u8>::uninit(); 6];
        f.read_exact_into_uninit_at(0, &mut buf).await.unwrap();
        // SAFETY: read_exact_into_uninit_at guarantees initialization on success.
        let initialized = unsafe { core::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), buf.len()) };
        assert_eq!(initialized, b"uninit");
    }
}
