// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Read-only sidecar for the frozen mimalloc 3.3.2 comparator. No option setters.

use std::ffi::{c_int, c_long};
use std::fs::File;
use std::io::Write;

// Exact mi_option_t order in libmimalloc-sys 0.1.49's bundled v3 header.
// Header SHA256: 869161433ab2d807a75752d0762accf5b68b6b24ce67e417af46bf181b9b6c72.
// Reject other runtime versions before passing these indices over the ABI.
const OPTIONS: [&str; 47] = [
    "show_errors",
    "show_stats",
    "verbose",
    "deprecated_eager_commit",
    "arena_eager_commit",
    "purge_decommits",
    "allow_large_os_pages",
    "reserve_huge_os_pages",
    "reserve_huge_os_pages_at",
    "reserve_os_memory",
    "deprecated_segment_cache",
    "deprecated_page_reset",
    "deprecated_abandoned_page_purge",
    "deprecated_segment_reset",
    "deprecated_eager_commit_delay",
    "purge_delay",
    "use_numa_nodes",
    "disallow_os_alloc",
    "os_tag",
    "max_errors",
    "max_warnings",
    "deprecated_max_segment_reclaim",
    "destroy_on_exit",
    "arena_reserve",
    "arena_purge_mult",
    "deprecated_purge_extend_delay",
    "disallow_arena_alloc",
    "retry_on_oom",
    "visit_abandoned",
    "guarded_min",
    "guarded_max",
    "guarded_precise",
    "guarded_sample_rate",
    "guarded_sample_seed",
    "generic_collect",
    "page_reclaim_on_free",
    "page_full_retain",
    "page_max_candidates",
    "max_vabits",
    "pagemap_commit",
    "page_commit_on_demand",
    "page_max_reclaim",
    "page_cross_thread_max_reclaim",
    "allow_thp",
    "minimal_purge_size",
    "arena_max_object_size",
    "arena_is_numa_local",
];

unsafe extern "C" {
    fn mi_version() -> c_int;
    fn mi_option_get(option: c_int) -> c_long;
}

pub(super) struct NativeProvenance {
    file: File,
}

impl NativeProvenance {
    pub(super) fn begin() -> Option<Self> {
        let path = std::env::var_os("RALLOCATOR_NATIVE_PROVENANCE_OUTPUT")?;
        let file = File::create_new(path).expect("each provenance run requires a new writable sidecar path");
        let mut provenance = Self { file };
        provenance.write("before");
        Some(provenance)
    }

    pub(super) fn finish(mut self) {
        self.write("after");
    }

    fn write(&mut self, phase: &str) {
        // SAFETY: The benchmark links mimalloc; this no-argument C ABI function
        // returns its native version and does not change allocator options.
        let version = unsafe { mi_version() };
        assert_eq!(version, 30302, "option indices are qualified only for the frozen 3.3.2 comparator");
        writeln!(self.file, "phase={phase} native_version={version} values=raw_mi_option_get_long")
            .expect("the explicitly configured provenance sidecar must remain writable");
        for (index, name) in OPTIONS.iter().enumerate() {
            let option = c_int::try_from(index).expect("the verified option table has only 47 entries");
            // SAFETY: The runtime version is checked above; every index is a
            // valid mi_option_t enumerator in its verified native header.
            let value = unsafe { mi_option_get(option) };
            writeln!(self.file, "option.{name}={value}").expect("the configured provenance sidecar must remain writable");
        }
    }
}
