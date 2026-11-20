// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
// Derived from mimalloc (39448a96), Copyright (c) Microsoft Research, Daan Leijen.
// See ../LICENSE-MIMALLOC.

pub(super) fn has_overcommit(setting: &[u8]) -> bool {
    setting.first().is_none_or(|value| matches!(value, b'0' | b'1'))
}

pub(super) fn has_transparent_huge_pages(setting: &[u8]) -> bool {
    !setting.is_empty() && !setting.windows(7).any(|value| value == b"[never]")
}

#[cfg(test)]
mod tests {
    use super::{has_overcommit, has_transparent_huge_pages};

    #[test]
    fn unavailable_overcommit_configuration_keeps_the_native_default() {
        assert!(has_overcommit(b""));
    }

    #[test]
    fn both_permissive_modes_allow_eager_commit() {
        assert_eq!((has_overcommit(b"0\n"), has_overcommit(b"1\n")), (true, true));
    }

    #[test]
    fn strict_and_unknown_overcommit_modes_disable_eager_commit() {
        assert_eq!((has_overcommit(b"2\n"), has_overcommit(b"3\n")), (false, false));
    }

    #[test]
    fn available_transparent_huge_pages_include_madvise_mode() {
        assert_eq!(
            (
                has_transparent_huge_pages(b"[always] madvise never\n"),
                has_transparent_huge_pages(b"always [madvise] never\n"),
            ),
            (true, true)
        );
    }

    #[test]
    fn disabled_or_unavailable_transparent_huge_pages_do_not_change_purge_size() {
        assert_eq!(
            (
                has_transparent_huge_pages(b"always madvise [never]\n"),
                has_transparent_huge_pages(b""),
            ),
            (false, false)
        );
    }
}
