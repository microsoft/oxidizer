# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

BeforeAll {
    . (Join-Path $PSScriptRoot '..\..\_common\TestHelpers.ps1')

    $script:RepoRoot = Get-OxiRepoRoot
    $script:AnvilVersionsFile = Join-Path $script:RepoRoot 'justfiles\anvil\versions.just'
    $script:SettingsTemplate = Join-Path $script:RepoRoot '.vscode\settings.template.jsonc'
}

Describe 'Pinned nightly toolchain' {
    It 'pins the same nightly in settings.template.jsonc as Cargo Anvil' {
        $anvilNightly = (
            Get-Content $script:AnvilVersionsFile |
                Select-String '^rust_nightly\s*:=\s*"([^"]+)"'
        ).Matches.Groups[1].Value
        $anvilNightly | Should -Match '^nightly-\d{4}-\d{2}-\d{2}$'

        $pins = [regex]::Matches((Get-Content $script:SettingsTemplate -Raw), 'nightly-\d{4}-\d{2}-\d{2}')
        $pins.Count | Should -BeGreaterThan 0 -Because 'the template pins rustfmt to a specific nightly'

        foreach ($pin in $pins) {
            $pin.Value | Should -Be $anvilNightly -Because 'the VS Code formatter must use Anvil''s pinned nightly'
        }
    }
}
