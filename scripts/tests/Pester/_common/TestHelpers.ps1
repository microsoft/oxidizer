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
# subtree). Resolves a fixed path relative to this helper's own location, so
# test files never need brittle `..\..\..` chains of their own.
function Get-OxiRepoRoot {
    return (Resolve-Path (Join-Path $PSScriptRoot '..\..\..\..')).Path
}
