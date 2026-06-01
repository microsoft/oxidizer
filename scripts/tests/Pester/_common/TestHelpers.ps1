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
#>

# Returns the repo root (the directory containing this scripts/tests/Pester
# subtree). Walks up from $env:OXI_TEST_COMMON (set by Run-Tests.ps1) so test
# files never need brittle `..\..\..` chains.
function Get-OxiRepoRoot {
    if ($env:OXI_TEST_COMMON) {
        return (Resolve-Path (Join-Path $env:OXI_TEST_COMMON '..\..\..\..')).Path
    }
    return (& git rev-parse --show-toplevel 2>&1).ToString().Trim()
}
