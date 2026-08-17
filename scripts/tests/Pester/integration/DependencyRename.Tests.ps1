# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

BeforeAll {
    . (Join-Path $PSScriptRoot '..\_common\TestHelpers.ps1')
    . (Join-Path $PSScriptRoot '..\_common\New-SyntheticWorkspace.ps1')
    . (Join-Path (Get-OxiRepoRoot) 'scripts\lib\releasing.ps1')

    $script:FactsScript = Join-Path (
        Get-OxiRepoRoot
    ) '.github\skills\release-packages\scripts\release-facts.ps1'

    $spec = @{
        Packages = @(
            @{ Name = 'dependency'; Version = '1.0.0'; LibName = 'dep_core' }
            @{
                Name = 'renamed_consumer'
                Version = '1.0.0'
                Deps = @(@{ Name = 'dependency'; Rename = 'aliased-dep' })
                AllowedExternalTypes = @('aliased_dep::Handle')
            }
            @{
                Name = 'lib_consumer'
                Version = '1.0.0'
                Deps = @(@{ Name = 'dependency' })
                AllowedExternalTypes = @('dep_core::Handle')
            }
            @{
                Name = 'shadowed_consumer'
                Version = '1.0.0'
                Deps = @(@{ Name = 'dependency'; Rename = 'aliased-dep' })
                AllowedExternalTypes = @('dep_core::Handle')
            }
            @{
                Name = 'relay'
                Version = '1.0.0'
                Deps = @(@{ Name = 'dependency' })
                AllowedExternalTypes = @()
            }
            @{
                Name = 'facade'
                Version = '1.0.0'
                Deps = @(@{ Name = 'relay' })
                AllowedExternalTypes = @('dep_core::Handle')
            }
        )
    }

    $script:Workspace = New-SyntheticWorkspace `
        -Spec $spec `
        -Path (Join-Path $TestDrive 'dependency-roots')
    Reset-ReleaseScriptCaches
    $script:Packages = @(Get-WorkspacePackages -repoRoot $script:Workspace.Path)
    $script:Facts = & $script:FactsScript -RepoRoot $script:Workspace.Path |
        ConvertFrom-Json
    $script:FactsByFolder = @{}
    foreach ($fact in $script:Facts.packages) {
        $script:FactsByFolder[$fact.folder] = $fact
    }
}

AfterAll {
    Get-ChildItem `
        -LiteralPath (Join-Path $script:Workspace.Path '.git') `
        -File `
        -Recurse `
        -Force |
        ForEach-Object { $_.IsReadOnly = $false }
}

Describe 'Dependency crate-root extraction' {
    It 'records a package rename under the real dependency name' {
        $consumer = $script:Packages |
            Where-Object Folder -eq 'renamed_consumer'

        $consumer.Deps | Should -Contain 'dependency'
        @($consumer.DepAliases['dependency']) | Should -Contain 'aliased_dep'
    }

    It 'records the dependency library target name when no edge rename exists' {
        $consumer = $script:Packages |
            Where-Object Folder -eq 'lib_consumer'

        @($consumer.DepAliases['dependency']) | Should -Contain 'dep_core'
    }

    It 'records each package own crate root' {
        $dependency = $script:Packages |
            Where-Object Folder -eq 'dependency'

        $dependency.CrateRoot | Should -Be 'dep_core'
    }
}

Describe 'Release facts for diverted crate roots' {
    It 'recognizes a direct exposure through a package rename' {
        @($script:FactsByFolder['renamed_consumer'].exposedDeps) |
            Should -Contain 'dependency'
    }

    It 'recognizes a direct exposure through a custom library target name' {
        @($script:FactsByFolder['lib_consumer'].exposedDeps) |
            Should -Contain 'dependency'
    }

    It 'does not use the library target name when an edge rename shadows it' {
        @($script:FactsByFolder['shadowed_consumer'].exposedDeps) |
            Should -Not -Contain 'dependency'
    }

    It 'recognizes an indirect re-export by the defining crate root' {
        $facade = $script:FactsByFolder['facade']

        @($facade.deps) | Should -Not -Contain 'dependency'
        @($facade.exposedDeps) | Should -Contain 'dependency'
    }
}
