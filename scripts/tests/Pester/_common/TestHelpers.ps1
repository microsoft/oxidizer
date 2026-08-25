# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

<#
.SYNOPSIS
    Common helpers for Pester tests. Dot-source from BeforeAll blocks.

.DESCRIPTION
    Provides Get-OxiRepoRoot for deterministic path resolution from any
    test file. Test files dot-source the shared script libraries
    (scripts/lib/release-flow.ps1 etc.) directly using
    Join-Path (Get-OxiRepoRoot) 'scripts\lib\<file>.ps1'.

    Also provides Get-BytesBufIoAllowlist, the one canonical copy of a real
    manifest literal shared by a unit test and the integration test that pins
    it against the manifest.
#>

# Returns the repo root (the directory containing this scripts/tests/Pester
# subtree). Resolves a fixed path relative to this helper's own location, so
# test files never need brittle `..\..\..` chains of their own.
function Get-OxiRepoRoot {
    return (Resolve-Path (Join-Path $PSScriptRoot '..\..\..\..')).Path
}

# Returns crates/bytesbuf_io's allowed_external_types, copied verbatim from its
# Cargo.toml.
#
# bytesbuf_io -> bytesbuf is the production instance the whole exposure cascade
# exists for, so its allowlist is asserted from two directions:
#
#   * unit  (PureFunctions.Tests.ps1) feeds this literal to
#     Test-PackageExposesTarget, proving the matching logic against a shape
#     that really occurs rather than an invented one; and
#   * integration (ExposureCascade-RealWorkspace.Tests.ps1) asserts the live
#     manifest still equals it exactly.
#
# It lives here so those two cannot drift apart: a second copy would let the
# unit test keep passing against a literal the manifest had already abandoned,
# which is the staleness the integration test is there to prevent.
function Get-BytesBufIoAllowlist {
    return @('bytesbuf::*', 'ohno::*', 'futures_core::stream::Stream')
}
