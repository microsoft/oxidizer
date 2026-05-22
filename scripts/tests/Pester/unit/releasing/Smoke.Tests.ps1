# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

BeforeAll {
    . (Join-Path $env:OXI_TEST_COMMON 'TestHelpers.ps1')
    . (Join-Path (Get-OxiRepoRoot) 'scripts\lib\releasing.ps1')
}

Describe 'Smoke: shared library loads' {
    It 'has Compare-SemanticVersions available' {
        Compare-SemanticVersions -version1 '1.2.3' -version2 '1.2.3' | Should -Be 0
        Compare-SemanticVersions -version1 '1.2.3' -version2 '1.2.4' | Should -Be -1
        Compare-SemanticVersions -version1 '1.2.4' -version2 '1.2.3' | Should -Be 1
    }

    It 'has Get-NextVersion available' {
        Get-NextVersion -currentVersion '1.2.3' -bump 'minor' | Should -Be '1.3.0'
    }
}
