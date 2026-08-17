# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

<#
    Tests for the release skill's thin deterministic changelog helper. Verifies
    the version header and the
    cascade "Now requires X of Y" bullet are written by reusing Write-Changelog.
#>

BeforeAll {
    . (Join-Path $PSScriptRoot '..\..\_common\TestHelpers.ps1')
    . (Join-Path $PSScriptRoot '..\..\_common\New-SyntheticWorkspace.ps1')
    . (Join-Path (Get-OxiRepoRoot) 'scripts\lib\releasing.ps1')

    $script:ChangelogScript = Join-Path (
        Get-OxiRepoRoot
    ) '.github\skills\release-packages\scripts\release-changelog.ps1'
}

Describe 'release-changelog.ps1' {
    BeforeEach {
        Reset-ReleaseScriptCaches
        $script:WsRoot = Join-Path $TestDrive ("cl-ws-" + [guid]::NewGuid().ToString('N'))
        $script:Ws = New-SyntheticWorkspace -Preset 'Linear2' -Path $script:WsRoot
    }

    It 'inserts a dated version header for the released package' {
        & $script:ChangelogScript -RepoRoot $script:WsRoot -PackageFolder 'dependency' -NewVersion '0.3.0' `
            -PrBaseUrl 'https://github.com/microsoft/oxidizer' 6>$null

        $changelog = Get-Content (Join-Path $script:WsRoot 'crates\dependency\CHANGELOG.md') -Raw
        $today = (Get-Date).ToString('yyyy-MM-dd')
        $changelog | Should -Match ([regex]::Escape("## [0.3.0] - $today"))
    }

    It 'writes cascade "Now requires" bullets from CascadeReasonsJson' {
        $reasons = '[{"Target":"dependency","Version":"0.3.0","Breaking":false}]'
        & $script:ChangelogScript -RepoRoot $script:WsRoot -PackageFolder 'dependent' -NewVersion '0.2.0' `
            -PrBaseUrl 'https://github.com/microsoft/oxidizer' -CascadeReasonsJson $reasons 6>$null

        $changelog = Get-Content (Join-Path $script:WsRoot 'crates\dependent\CHANGELOG.md') -Raw
        $changelog | Should -Match ([regex]::Escape('## [0.2.0] -'))
        $changelog | Should -Match ([regex]::Escape('Now requires `0.3.0` of `dependency`'))
    }

    It 'renders a breaking cascade under the Breaking section' {
        $reasons = '[{"Target":"dependency","Version":"1.0.0","Breaking":true}]'
        & $script:ChangelogScript -RepoRoot $script:WsRoot -PackageFolder 'dependent' -NewVersion '0.2.0' `
            -PrBaseUrl 'https://github.com/microsoft/oxidizer' -CascadeReasonsJson $reasons 6>$null

        $changelog = Get-Content (Join-Path $script:WsRoot 'crates\dependent\CHANGELOG.md') -Raw
        $changelog | Should -Match ([regex]::Escape('Breaking'))
        $changelog | Should -Match ([regex]::Escape('Now requires `1.0.0` of `dependency`'))
    }

    It 'fails clearly for an unknown package folder' {
        { & $script:ChangelogScript -RepoRoot $script:WsRoot -PackageFolder 'does_not_exist' -NewVersion '0.2.0' 6>$null } |
            Should -Throw '*was not found under*'
    }
}
