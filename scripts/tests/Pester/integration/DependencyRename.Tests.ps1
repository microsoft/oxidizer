# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

# Covers the exposure paths where a dependency's crate root differs from its
# package name, end-to-end from a real Cargo manifest through `cargo metadata`
# to the cascade decision. Two constructs do this: `package = "..."` on the
# dependency (rename) and `[lib] name = "..."` in the dependency's own manifest.
#
# The unit tests for this construct DepAliases by hand, already normalized, so
# they prove the matching logic but not the extraction that feeds it. A
# regression in reading `dependency.rename` or the lib target name, or in
# converting `aliased-dep` to `aliased_dep`, would restore the original
# fail-open with every unit test still green. These tests close that gap by
# building actual workspaces and loading them through Get-WorkspacePackages.

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

    # `dependency` renames its own crate root with `[lib] name = "dep_core"`.
    # The package is still depended on as `dependency`, but Rust source -- and
    # therefore the allowlist -- can only name it as `dep_core`.
    function New-LibNameWorkspace {
        param(
            [Parameter(Mandatory = $true)][string]$Path,
            [string[]]$AllowedExternalTypes = @('dep_core::Handle'),
            [string]$Rename = $null
        )
        $dep = @{ Name = 'dependency' }
        if (-not [string]::IsNullOrWhiteSpace($Rename)) { $dep['Rename'] = $Rename }
        $spec = @{
            Packages = @(
                @{
                    Name = 'dependent'; Version = '1.0.0'
                    Deps = @($dep)
                    AllowedExternalTypes = $AllowedExternalTypes
                }
                @{ Name = 'dependency'; Version = '1.0.0'; LibName = 'dep_core' }
            )
        }
        return New-SyntheticWorkspace -Spec $spec -Path $Path
    }
    # defining -> relay -> facade, where defining's crate root is `def_core`
    # and facade reaches a def_core type re-exported through relay. facade
    # declares no edge to defining, so nothing on any edge it owns can tell it
    # what defining's crate root is -- the root has to come from the target.
    function New-IndirectLibNameWorkspace {
        param(
            [Parameter(Mandatory = $true)][string]$Path,
            [string[]]$FacadeAllowedExternalTypes = @('def_core::Handle')
        )
        $spec = @{
            Packages = @(
                @{
                    Name = 'facade'; Version = '1.0.0'
                    Deps = @(@{ Name = 'relay' })
                    AllowedExternalTypes = $FacadeAllowedExternalTypes
                }
                @{
                    Name = 'relay'; Version = '1.0.0'
                    Deps = @(@{ Name = 'defining' })
                    AllowedExternalTypes = @()
                }
                @{ Name = 'defining'; Version = '1.0.0'; LibName = 'def_core'; AllowedExternalTypes = @() }
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

Describe 'Renamed crate root via [lib] name (via cargo metadata)' {
    BeforeEach {
        Reset-ReleaseScriptCaches
    }

    It 'records the dependency''s lib target name as an alias' {
        $ws = New-LibNameWorkspace -Path (Join-Path $TestDrive 'libname-extract')
        $pkgs = @(Get-WorkspacePackages -repoRoot $ws.Path)
        $dependent = $pkgs | Where-Object { $_.Folder -eq 'dependent' }

        # The edge is still keyed by the package name; the crate root is extra.
        $dependent.Deps | Should -Contain 'dependency'
        $dependent.DepAliases.ContainsKey('dependency') | Should -BeTrue
        @($dependent.DepAliases['dependency']) | Should -Contain 'dep_core'
    }

    It 'records no alias when the lib target name matches the package name' {
        # Negative control for the extraction: the alias must come from a real
        # divergence, not be manufactured for every dependency.
        $ws = New-RenamedDepWorkspace -Path (Join-Path $TestDrive 'libname-matching')
        $pkgs = @(Get-WorkspacePackages -repoRoot $ws.Path)
        $dependency = $pkgs | Where-Object { $_.Folder -eq 'dependency' }

        $dependency.DepAliases.Count | Should -Be 0
    }

    It 'reports exposure for an allowlist entry rooted at the lib name' {
        $ws = New-LibNameWorkspace -Path (Join-Path $TestDrive 'libname-exposes')
        $pkgs = @(Get-WorkspacePackages -repoRoot $ws.Path)
        $dependent = $pkgs | Where-Object { $_.Folder -eq 'dependent' }

        Test-PackageExposesTarget -Dependent $dependent -TargetPackageName 'dependency' | Should -BeTrue
    }

    It 'reports no exposure when the allowlist names neither the lib name nor the package' {
        $ws = New-LibNameWorkspace -Path (Join-Path $TestDrive 'libname-unrelated') `
            -AllowedExternalTypes @('unrelated_crate::Handle')
        $pkgs = @(Get-WorkspacePackages -repoRoot $ws.Path)
        $dependent = $pkgs | Where-Object { $_.Folder -eq 'dependent' }

        Test-PackageExposesTarget -Dependent $dependent -TargetPackageName 'dependency' | Should -BeFalse
    }

    It 'prefers the rename over the lib name when the dependency declares both' {
        # `aliased-dep = { package = "dependency" }` against a dependency whose
        # lib target is `dep_core`. Rust names it `aliased_dep`: the rename is
        # applied to the dependency edge, so the lib name never surfaces in the
        # consumer. Recording `dep_core` here would be inert at best and, for a
        # crate that allowlists `dep_core` through some *other* path, a false
        # exposure on this edge.
        $ws = New-LibNameWorkspace -Path (Join-Path $TestDrive 'libname-rename-wins') `
            -Rename 'aliased-dep' -AllowedExternalTypes @('aliased_dep::Handle')
        $pkgs = @(Get-WorkspacePackages -repoRoot $ws.Path)
        $dependent = $pkgs | Where-Object { $_.Folder -eq 'dependent' }

        @($dependent.DepAliases['dependency']) | Should -Contain 'aliased_dep'
        @($dependent.DepAliases['dependency']) | Should -Not -Contain 'dep_core'
        Test-PackageExposesTarget -Dependent $dependent -TargetPackageName 'dependency' | Should -BeTrue
    }

    It 'does not accept the lib name on an edge where a rename shadows it' {
        # The other half of the precedence rule, and the half that actually
        # bites. The test above pins what DepAliases *contains*; this one pins
        # what the predicate *does*, which is what a regression breaks. A
        # dependent importing the crate as `aliased_dep` cannot write
        # `dep_core::Handle` -- the rename shadows the lib name completely -- so
        # such an entry must be some unrelated crate and must not count as
        # exposure here. Reintroducing the target's global crate root on a
        # declared edge turns that collision into a spurious breaking bump.
        $ws = New-LibNameWorkspace -Path (Join-Path $TestDrive 'libname-rename-shadows') `
            -Rename 'aliased-dep' -AllowedExternalTypes @('dep_core::Handle')
        $baseline = @(Get-WorkspacePackages -repoRoot $ws.Path)
        $dependent = $baseline | Where-Object { $_.Folder -eq 'dependent' }

        Test-PackageExposesTarget -Dependent $dependent -TargetPackageName 'dependency' | Should -BeFalse

        $stub = { param([string]$Folder, [string]$CargoName) 'none' }
        $resolved = Resolve-ReleaseSet `
            -ParsedTokens (Parse-ReleaseTokens -Tokens @('dependency@breaking')) `
            -WorkspaceBaseline $baseline `
            -GetRequiredChangeType $stub
        $resolvedDependent = $resolved | Where-Object { $_.Folder -eq 'dependent' }

        $resolvedDependent.EffectiveChangeType    | Should -Be 'patch'
        $resolvedDependent.EffectiveTargetVersion | Should -Be '1.0.1'
    }

    It 'cascades a breaking dependency through the lib-name exposure edge' {
        $ws = New-LibNameWorkspace -Path (Join-Path $TestDrive 'libname-cascade')
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

    It 'does not cascade when the lib-name root is absent from the allowlist' {
        $ws = New-LibNameWorkspace -Path (Join-Path $TestDrive 'libname-nocascade') `
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

Describe 'Indirect exposure of a diverted crate root (via cargo metadata)' {
    BeforeEach {
        Reset-ReleaseScriptCaches
    }

    It 'cascades to an indirect dependent whose allowlist is rooted at the lib name' {
        $ws = New-IndirectLibNameWorkspace -Path (Join-Path $TestDrive 'indirect-libname-cascade')
        $baseline = @(Get-WorkspacePackages -repoRoot $ws.Path)
        $stub = { param([string]$Folder, [string]$CargoName) 'none' }

        $resolved = Resolve-ReleaseSet `
            -ParsedTokens (Parse-ReleaseTokens -Tokens @('defining@breaking')) `
            -WorkspaceBaseline $baseline `
            -GetRequiredChangeType $stub
        $facade = $resolved | Where-Object { $_.Folder -eq 'facade' }

        $facade.EffectiveChangeType    | Should -Be 'breaking'
        $facade.EffectiveTargetVersion | Should -Be '2.0.0'
        # relay claims to expose nothing, so it correctly stays at its floor --
        # facade is reached on its own allowlist evidence, not through relay.
        ($resolved | Where-Object { $_.Folder -eq 'relay' }).EffectiveChangeType | Should -Be 'patch'
    }

    It 'does not cascade to an indirect dependent that names an unrelated root' {
        # Negative control: proves the test above passes because the allowlist
        # names defining's crate root, not because the indirect branch fails open.
        $ws = New-IndirectLibNameWorkspace -Path (Join-Path $TestDrive 'indirect-libname-nocascade') `
            -FacadeAllowedExternalTypes @('unrelated_crate::Handle')
        $baseline = @(Get-WorkspacePackages -repoRoot $ws.Path)
        $stub = { param([string]$Folder, [string]$CargoName) 'none' }

        $resolved = Resolve-ReleaseSet `
            -ParsedTokens (Parse-ReleaseTokens -Tokens @('defining@breaking')) `
            -WorkspaceBaseline $baseline `
            -GetRequiredChangeType $stub
        $facade = $resolved | Where-Object { $_.Folder -eq 'facade' }

        $facade.EffectiveChangeType    | Should -Be 'patch'
        $facade.EffectiveTargetVersion | Should -Be '1.0.1'
    }
}
