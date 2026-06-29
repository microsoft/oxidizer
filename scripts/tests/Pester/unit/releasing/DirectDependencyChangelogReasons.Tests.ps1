# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
#
# Unit tests for Get-DirectDependencyChangelogReasons (ADO bug 7536096).
#
# A dependent's changelog "Now requires <version> of <target>" bullets must name
# only the DIRECT workspace dependencies declared in that crate's own Cargo.toml
# (normal/build, dev excluded) that are part of THIS release with a changed
# version — each at its NEW version. This is deliberately decoupled from the
# entry's CascadeReasons (which attribute a cascade to its root-cause crate for
# pin-conflict / plan diagnostics). The regression these tests pin: an INDIRECT
# dependent used to get a bullet naming the root-cause crate it does not directly
# depend on.

BeforeAll {
    . (Join-Path $PSScriptRoot '..\..\_common\TestHelpers.ps1')
    . (Join-Path (Get-OxiRepoRoot) 'scripts\lib\release-flow.ps1')

    function New-BaselinePackage {
        param(
            [string]   $Folder,
            [string]   $Name = $null,
            [string]   $Version = '0.1.0',
            [string[]] $Deps = @(),
            [bool]     $Published = $true
        )
        if ([string]::IsNullOrEmpty($Name)) { $Name = $Folder }
        return [pscustomobject]@{
            Folder    = $Folder
            Name      = $Name
            Version   = $Version
            Published = $Published
            Deps      = $Deps
        }
    }

    function New-ResolvedEntry {
        param(
            [string] $Folder,
            [string] $Name = $null,
            [string] $CurrentVersion,
            [string] $EffectiveTargetVersion,
            [string] $EffectiveChangeType = 'non-breaking',
            [object[]] $CascadeReasons = @()
        )
        if ([string]::IsNullOrEmpty($Name)) { $Name = $Folder }
        $reasons = New-Object 'System.Collections.Generic.List[object]'
        foreach ($r in $CascadeReasons) { [void]$reasons.Add($r) }
        return [pscustomobject]@{
            Folder                 = $Folder
            Name                   = $Name
            CurrentVersion         = $CurrentVersion
            EffectiveTargetVersion = $EffectiveTargetVersion
            EffectiveChangeType    = $EffectiveChangeType
            CascadeReasons         = $reasons
        }
    }

    function ConvertTo-ResolvedHash {
        param([object[]]$Entries)
        $h = @{}
        foreach ($e in $Entries) { $h[$e.Folder] = $e }
        return $h
    }
}

Describe 'Get-DirectDependencyChangelogReasons' {
    Context 'indirect dependent (ADO bug 7536096 repro shape)' {
        # thread_aware_macros_impl <- thread_aware <- bytesbuf
        # Release thread_aware_macros_impl@patch cascades to thread_aware AND
        # bytesbuf. bytesbuf's Cargo.toml depends ONLY on thread_aware, so its
        # changelog must name thread_aware (at its NEW version), never the
        # root-cause thread_aware_macros_impl.
        BeforeEach {
            $script:Baseline = @(
                (New-BaselinePackage -Folder 'thread_aware_macros_impl' -Version '0.7.3' -Deps @())
                (New-BaselinePackage -Folder 'thread_aware'             -Version '0.7.4' -Deps @('thread_aware_macros_impl'))
                (New-BaselinePackage -Folder 'bytesbuf'                 -Version '0.5.0' -Deps @('thread_aware'))
            )
            $script:Resolved = ConvertTo-ResolvedHash @(
                (New-ResolvedEntry -Folder 'thread_aware_macros_impl' -CurrentVersion '0.7.3' -EffectiveTargetVersion '0.7.4' -EffectiveChangeType 'patch')
                (New-ResolvedEntry -Folder 'thread_aware'             -CurrentVersion '0.7.4' -EffectiveTargetVersion '0.7.5' -EffectiveChangeType 'patch')
                (New-ResolvedEntry -Folder 'bytesbuf'                 -CurrentVersion '0.5.0' -EffectiveTargetVersion '0.5.1' -EffectiveChangeType 'patch')
            )
        }

        It 'names only the DIRECT dependency at its new version, not the root cause' {
            $entry  = $script:Resolved['bytesbuf']
            $result = @(Get-DirectDependencyChangelogReasons -Entry $entry -ResolvedReleaseSet $script:Resolved -WorkspaceBaseline $script:Baseline)

            $result.Count            | Should -Be 1
            $result[0].Target        | Should -Be 'thread_aware'
            $result[0].Version       | Should -Be '0.7.5'
            $result.Target           | Should -Not -Contain 'thread_aware_macros_impl'
        }

        It 'names the direct dependency at its new version for the middle crate too' {
            $entry  = $script:Resolved['thread_aware']
            $result = @(Get-DirectDependencyChangelogReasons -Entry $entry -ResolvedReleaseSet $script:Resolved -WorkspaceBaseline $script:Baseline)

            $result.Count      | Should -Be 1
            $result[0].Target  | Should -Be 'thread_aware_macros_impl'
            $result[0].Version | Should -Be '0.7.4'
        }

        It 'emits no bullet for the released root crate (no direct deps in the set)' {
            $entry  = $script:Resolved['thread_aware_macros_impl']
            $result = @(Get-DirectDependencyChangelogReasons -Entry $entry -ResolvedReleaseSet $script:Resolved -WorkspaceBaseline $script:Baseline)

            $result.Count | Should -Be 0
        }
    }

    Context 'multiple direct dependencies' {
        It 'emits one reason per direct dependency that changed, each at its new version' {
            $baseline = @(
                (New-BaselinePackage -Folder 'consumer' -Version '0.1.0' -Deps @('lib_a', 'lib_b', 'lib_c'))
                (New-BaselinePackage -Folder 'lib_a'    -Version '0.2.0')
                (New-BaselinePackage -Folder 'lib_b'    -Version '0.3.0')
                (New-BaselinePackage -Folder 'lib_c'    -Version '0.4.0')
            )
            $resolved = ConvertTo-ResolvedHash @(
                (New-ResolvedEntry -Folder 'consumer' -CurrentVersion '0.1.0' -EffectiveTargetVersion '0.1.1')
                (New-ResolvedEntry -Folder 'lib_a'    -CurrentVersion '0.2.0' -EffectiveTargetVersion '0.2.1')
                (New-ResolvedEntry -Folder 'lib_b'    -CurrentVersion '0.3.0' -EffectiveTargetVersion '0.3.1')
                (New-ResolvedEntry -Folder 'lib_c'    -CurrentVersion '0.4.0' -EffectiveTargetVersion '0.4.1')
            )

            $result = @(Get-DirectDependencyChangelogReasons -Entry $resolved['consumer'] -ResolvedReleaseSet $resolved -WorkspaceBaseline $baseline)

            $result.Count | Should -Be 3
            ($result | ForEach-Object { "$($_.Target)@$($_.Version)" } | Sort-Object) |
                Should -Be @('lib_a@0.2.1', 'lib_b@0.3.1', 'lib_c@0.4.1')
        }
    }

    Context 'filtering' {
        It 'excludes a direct dependency whose version did not change' {
            $baseline = @(
                (New-BaselinePackage -Folder 'consumer' -Version '0.1.0' -Deps @('changed', 'unchanged'))
                (New-BaselinePackage -Folder 'changed'   -Version '0.2.0')
                (New-BaselinePackage -Folder 'unchanged' -Version '0.3.0')
            )
            $resolved = ConvertTo-ResolvedHash @(
                (New-ResolvedEntry -Folder 'consumer'  -CurrentVersion '0.1.0' -EffectiveTargetVersion '0.1.1')
                (New-ResolvedEntry -Folder 'changed'   -CurrentVersion '0.2.0' -EffectiveTargetVersion '0.2.1')
                (New-ResolvedEntry -Folder 'unchanged' -CurrentVersion '0.3.0' -EffectiveTargetVersion '0.3.0')
            )

            $result = @(Get-DirectDependencyChangelogReasons -Entry $resolved['consumer'] -ResolvedReleaseSet $resolved -WorkspaceBaseline $baseline)

            $result.Count     | Should -Be 1
            $result[0].Target | Should -Be 'changed'
        }

        It 'excludes a direct dependency that is not part of the release set' {
            $baseline = @(
                (New-BaselinePackage -Folder 'consumer' -Version '0.1.0' -Deps @('in_set', 'serde'))
                (New-BaselinePackage -Folder 'in_set'   -Version '0.2.0')
            )
            $resolved = ConvertTo-ResolvedHash @(
                (New-ResolvedEntry -Folder 'consumer' -CurrentVersion '0.1.0' -EffectiveTargetVersion '0.1.1')
                (New-ResolvedEntry -Folder 'in_set'   -CurrentVersion '0.2.0' -EffectiveTargetVersion '0.2.1')
            )

            $result = @(Get-DirectDependencyChangelogReasons -Entry $resolved['consumer'] -ResolvedReleaseSet $resolved -WorkspaceBaseline $baseline)

            $result.Count     | Should -Be 1
            $result[0].Target | Should -Be 'in_set'
        }

        It 'returns empty when the dependent has no direct deps in the release set' {
            $baseline = @(
                (New-BaselinePackage -Folder 'consumer' -Version '0.1.0' -Deps @('external_only'))
            )
            $resolved = ConvertTo-ResolvedHash @(
                (New-ResolvedEntry -Folder 'consumer' -CurrentVersion '0.1.0' -EffectiveTargetVersion '0.1.1')
            )

            $result = @(Get-DirectDependencyChangelogReasons -Entry $resolved['consumer'] -ResolvedReleaseSet $resolved -WorkspaceBaseline $baseline)

            $result.Count | Should -Be 0
        }
    }

    Context 'name normalization' {
        It 'reports the dependency cargo name (with dashes), resolving via underscore-normalized deps' {
            # Baseline .Deps store the underscore-normalized name; the resolved
            # entry carries the dashed cargo name. The bullet must use the cargo
            # name as declared.
            $baseline = @(
                (New-BaselinePackage -Folder 'http_server' -Name 'http-server' -Version '0.1.0' -Deps @('http_layer'))
                (New-BaselinePackage -Folder 'http_layer'  -Name 'http-layer'  -Version '0.2.0')
            )
            $resolved = ConvertTo-ResolvedHash @(
                (New-ResolvedEntry -Folder 'http_server' -Name 'http-server' -CurrentVersion '0.1.0' -EffectiveTargetVersion '0.1.1')
                (New-ResolvedEntry -Folder 'http_layer'  -Name 'http-layer'  -CurrentVersion '0.2.0' -EffectiveTargetVersion '0.2.1')
            )

            $result = @(Get-DirectDependencyChangelogReasons -Entry $resolved['http_server'] -ResolvedReleaseSet $resolved -WorkspaceBaseline $baseline)

            $result.Count      | Should -Be 1
            $result[0].Target  | Should -Be 'http-layer'
            $result[0].Version | Should -Be '0.2.1'
        }
    }

    Context 'breaking flag drives section selection (preserves old per-edge aggregate)' {
        It 'marks reasons Breaking when an edge in the entry''s CascadeReasons is breaking' {
            $baseline = @(
                (New-BaselinePackage -Folder 'app' -Version '1.2.0' -Deps @('engine'))
                (New-BaselinePackage -Folder 'engine' -Version '2.0.0')
            )
            $resolved = ConvertTo-ResolvedHash @(
                (New-ResolvedEntry -Folder 'app' -CurrentVersion '1.2.0' -EffectiveTargetVersion '2.0.0' -EffectiveChangeType 'breaking' `
                    -CascadeReasons @([pscustomobject]@{ Target = 'engine'; Breaking = $true }))
                (New-ResolvedEntry -Folder 'engine' -CurrentVersion '2.0.0' -EffectiveTargetVersion '3.0.0' -EffectiveChangeType 'breaking')
            )

            $result = @(Get-DirectDependencyChangelogReasons -Entry $resolved['app'] -ResolvedReleaseSet $resolved -WorkspaceBaseline $baseline)

            $result.Count       | Should -Be 1
            $result[0].Breaking | Should -BeTrue
        }

        It 'leaves reasons non-breaking when every edge in CascadeReasons is non-breaking' {
            $baseline = @(
                (New-BaselinePackage -Folder 'app' -Version '1.2.0' -Deps @('engine'))
                (New-BaselinePackage -Folder 'engine' -Version '2.0.0')
            )
            $resolved = ConvertTo-ResolvedHash @(
                (New-ResolvedEntry -Folder 'app' -CurrentVersion '1.2.0' -EffectiveTargetVersion '1.3.0' -EffectiveChangeType 'non-breaking' `
                    -CascadeReasons @([pscustomobject]@{ Target = 'engine'; Breaking = $false }))
                (New-ResolvedEntry -Folder 'engine' -CurrentVersion '2.0.0' -EffectiveTargetVersion '3.0.0' -EffectiveChangeType 'breaking')
            )

            $result = @(Get-DirectDependencyChangelogReasons -Entry $resolved['app'] -ResolvedReleaseSet $resolved -WorkspaceBaseline $baseline)

            $result.Count       | Should -Be 1
            $result[0].Breaking | Should -BeFalse
        }

        It 'keeps the bullet non-breaking for a crate that is a BREAKING user target but a NON-BREAKING cascade dependent (ADO 7536096 reviewer counterexample)' {
            # dependent is released breaking via its OWN request, but the cascade
            # edge dependency->dependent is non-breaking. The "Now requires" bullet
            # must land under Maintenance (Breaking=$false), driven by the per-edge
            # CascadeReasons flag — NOT by the dependent's own breaking change type.
            $baseline = @(
                (New-BaselinePackage -Folder 'dependent'  -Version '1.0.0' -Deps @('dependency'))
                (New-BaselinePackage -Folder 'dependency' -Version '0.2.0')
            )
            $resolved = ConvertTo-ResolvedHash @(
                (New-ResolvedEntry -Folder 'dependent' -CurrentVersion '1.0.0' -EffectiveTargetVersion '2.0.0' -EffectiveChangeType 'breaking' `
                    -CascadeReasons @([pscustomobject]@{ Target = 'dependency'; Breaking = $false }))
                (New-ResolvedEntry -Folder 'dependency' -CurrentVersion '0.2.0' -EffectiveTargetVersion '0.2.1' -EffectiveChangeType 'patch')
            )

            $result = @(Get-DirectDependencyChangelogReasons -Entry $resolved['dependent'] -ResolvedReleaseSet $resolved -WorkspaceBaseline $baseline)

            $result.Count       | Should -Be 1
            $result[0].Target   | Should -Be 'dependency'
            $result[0].Version  | Should -Be '0.2.1'
            $result[0].Breaking | Should -BeFalse -Because 'section selection follows the per-edge cascade flag, not the dependent''s own change type'
        }
    }
}
