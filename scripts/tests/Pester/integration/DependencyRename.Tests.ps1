# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

# Covers the renamed-dependency exposure path end-to-end, from a real Cargo
# manifest through `cargo metadata` to the cascade decision.
#
# The unit tests for this construct DepAliases by hand, already normalized, so
# they prove the matching logic but not the extraction that feeds it. A
# regression in reading `dependency.rename`, or in converting `aliased-dep` to
# `aliased_dep`, would restore the original fail-open with every unit test
# still green. These tests close that gap by building an actual workspace with
# `package = "..."` and loading it through Get-WorkspacePackages.

BeforeAll {
    . (Join-Path $PSScriptRoot '..\_common\TestHelpers.ps1')
    . (Join-Path $PSScriptRoot '..\_common\New-SyntheticWorkspace.ps1')
    . (Join-Path (Get-OxiRepoRoot) 'scripts\lib\release-flow.ps1')

    # dependent depends on `dependency` but declares it under the alias
    # `aliased-dep`, so Rust source -- and therefore the allowlist -- can only
    # name it as `aliased_dep`.
    function New-RenamedDepWorkspace {
        param(
            [Parameter(Mandatory = $true)][string]$Path,
            [string[]]$AllowedExternalTypes = @('aliased_dep::Handle')
        )
        $spec = @{
            Packages = @(
                @{
                    Name = 'dependent'; Version = '1.0.0'
                    Deps = @(@{ Name = 'dependency'; Rename = 'aliased-dep' })
                    AllowedExternalTypes = $AllowedExternalTypes
                }
                @{ Name = 'dependency'; Version = '1.0.0' }
            )
        }
        return New-SyntheticWorkspace -Spec $spec -Path $Path
    }
}

Describe 'Renamed dependency exposure (via cargo metadata)' {
    BeforeEach {
        Reset-ReleaseScriptCaches
    }

    It 'records the rename alias from cargo metadata, normalized to underscores' {
        $ws = New-RenamedDepWorkspace -Path (Join-Path $TestDrive 'rename-extract')
        $pkgs = @(Get-WorkspacePackages -repoRoot $ws.Path)
        $dependent = $pkgs | Where-Object { $_.Folder -eq 'dependent' }

        # The edge is keyed by the REAL package name; only the alias is extra.
        $dependent.Deps | Should -Contain 'dependency'
        $dependent.DepAliases.ContainsKey('dependency') | Should -BeTrue
        # 'aliased-dep' in the manifest, 'aliased_dep' in Rust paths.
        @($dependent.DepAliases['dependency']) | Should -Contain 'aliased_dep'
    }

    It 'leaves DepAliases empty for a package with no renamed dependencies' {
        $ws = New-RenamedDepWorkspace -Path (Join-Path $TestDrive 'rename-none')
        $pkgs = @(Get-WorkspacePackages -repoRoot $ws.Path)
        $dependency = $pkgs | Where-Object { $_.Folder -eq 'dependency' }

        $dependency.DepAliases.Count | Should -Be 0
    }

    It 'reports exposure for an allowlist entry rooted at the alias' {
        $ws = New-RenamedDepWorkspace -Path (Join-Path $TestDrive 'rename-exposes')
        $pkgs = @(Get-WorkspacePackages -repoRoot $ws.Path)
        $dependent = $pkgs | Where-Object { $_.Folder -eq 'dependent' }

        Test-PackageExposesTarget -Dependent $dependent -TargetPackageName 'dependency' | Should -BeTrue
    }

    It 'reports no exposure when the allowlist names neither the alias nor the real name' {
        # Negative control: proves the test above passes because of the alias
        # and not because something upstream fails closed.
        $ws = New-RenamedDepWorkspace -Path (Join-Path $TestDrive 'rename-unrelated') `
            -AllowedExternalTypes @('unrelated_crate::Handle')
        $pkgs = @(Get-WorkspacePackages -repoRoot $ws.Path)
        $dependent = $pkgs | Where-Object { $_.Folder -eq 'dependent' }

        Test-PackageExposesTarget -Dependent $dependent -TargetPackageName 'dependency' | Should -BeFalse
    }

    It 'cascades a breaking dependency through the aliased exposure edge' {
        $ws = New-RenamedDepWorkspace -Path (Join-Path $TestDrive 'rename-cascade')
        $baseline = @(Get-WorkspacePackages -repoRoot $ws.Path)
        $stub = { param([string]$Folder, [string]$CargoName) 'none' }

        $resolved = Resolve-ReleaseSet `
            -ParsedTokens (Parse-ReleaseTokens -Tokens @('dependency@breaking')) `
            -WorkspaceBaseline $baseline `
            -GetRequiredChangeType $stub
        $dependent = $resolved | Where-Object { $_.Folder -eq 'dependent' }

        $dependent.EffectiveChangeType    | Should -Be 'breaking'
        $dependent.EffectiveTargetVersion | Should -Be '2.0.0'
    }

    It 'does not cascade when the aliased dependency is absent from the allowlist' {
        $ws = New-RenamedDepWorkspace -Path (Join-Path $TestDrive 'rename-nocascade') `
            -AllowedExternalTypes @('unrelated_crate::Handle')
        $baseline = @(Get-WorkspacePackages -repoRoot $ws.Path)
        $stub = { param([string]$Folder, [string]$CargoName) 'none' }

        $resolved = Resolve-ReleaseSet `
            -ParsedTokens (Parse-ReleaseTokens -Tokens @('dependency@breaking')) `
            -WorkspaceBaseline $baseline `
            -GetRequiredChangeType $stub
        $dependent = $resolved | Where-Object { $_.Folder -eq 'dependent' }

        $dependent.EffectiveChangeType    | Should -Be 'patch'
        $dependent.EffectiveTargetVersion | Should -Be '1.0.1'
    }
}
