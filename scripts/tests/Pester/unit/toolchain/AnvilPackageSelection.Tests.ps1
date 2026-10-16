# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

BeforeAll {
    . (Join-Path $PSScriptRoot '..\..\_common\TestHelpers.ps1')
    . (Join-Path (Get-OxiRepoRoot) 'scripts\lib\anvil-package-selection.ps1')
}

Describe 'Anvil workspace package selection' {
    BeforeEach {
        $local = 'path+file:///checkout%20with%20spaces/crates/ohno#0.5.2'
        $registry = 'registry+https://github.com/rust-lang/crates.io-index#ohno@0.5.2'
        $metadata = [pscustomobject]@{
            workspace_members = @($local)
            packages = @(
                [pscustomobject]@{ name = 'ohno'; version = '0.5.2'; id = $registry }
                [pscustomobject]@{ name = 'ohno'; version = '0.5.2'; id = $local }
            )
        }
    }

    It 'selects the local source when name and version match a registry package' {
        ConvertTo-WorkspacePackageSelection '--package ohno@0.5.2' $metadata |
            Should -Be '--package path+file:///checkout%20with%20spaces/crates/ohno#ohno@0.5.2'
    }

    It 'resolves a portable cached selection using the consuming checkout path' {
        $metadata.workspace_members = @('path+file:///D:/runner/oxidizer/crates/ohno#0.5.2')
        $metadata.packages[1].id = $metadata.workspace_members[0]
        ConvertTo-WorkspacePackageSelection '--package ohno@0.5.2' $metadata |
            Should -Be '--package path+file:///D:/runner/oxidizer/crates/ohno#ohno@0.5.2'
    }

    It 'uses the same source-qualified identity for doctest capability selections' {
        ConvertTo-WorkspacePackageSelection 'ohno@0.5.2' $metadata |
            Should -Be 'path+file:///checkout%20with%20spaces/crates/ohno#ohno@0.5.2'
    }

    It 'qualifies a bare explicit workspace package override' {
        ConvertTo-WorkspacePackageSelection '--package ohno' $metadata |
            Should -Be '--package path+file:///checkout%20with%20spaces/crates/ohno#ohno@0.5.2'
    }

    It 'uses metadata IDs for tools that compare the original package ID fragment' {
        ConvertTo-WorkspacePackageSelection '--package ohno@0.5.2' $metadata -Format MetadataId |
            Should -Be "--package $local"
    }

    It 'converts an already qualified selection at a tool boundary' {
        ConvertTo-WorkspacePackageSelection '--package path+file:///checkout%20with%20spaces/crates/ohno#ohno@0.5.2' $metadata -Format MetadataId |
            Should -Be "--package $local"
    }

    It 'uses name and version only for workspace-only selectors such as cargo-each' {
        ConvertTo-WorkspacePackageSelection '--package path+file:///checkout%20with%20spaces/crates/ohno#ohno@0.5.2' $metadata -Format NameVersion |
            Should -Be '--package ohno@0.5.2'
    }

    It 'preserves metadata IDs that already include the package name' {
        $metadata.workspace_members = @('path+file:///checkout/crates/errors#ohno@0.5.2')
        $metadata.packages[1].id = $metadata.workspace_members[0]
        ConvertTo-WorkspacePackageSelection '--package ohno' $metadata -Format MetadataId |
            Should -Be "--package $($metadata.workspace_members[0])"
    }

    It 'does not accept a registry source as a workspace override' {
        { ConvertTo-WorkspacePackageSelection "--package $registry" $metadata } |
            Should -Throw '*unknown workspace*'
    }

    It 'preserves the <Selection> sentinel' -TestCases @(
        @{ Selection = '--skip' }
        @{ Selection = '--workspace' }
        @{ Selection = '' }
    ) {
        param($Selection)
        ConvertTo-WorkspacePackageSelection $Selection $metadata | Should -Be $Selection
    }

    It 'rejects unknown or incomplete package selections' {
        { ConvertTo-WorkspacePackageSelection '--package unknown@1.0.0' $metadata } | Should -Throw '*unknown workspace*'
        { ConvertTo-WorkspacePackageSelection '--package' $metadata } | Should -Throw '*missing its package spec*'
    }
}
