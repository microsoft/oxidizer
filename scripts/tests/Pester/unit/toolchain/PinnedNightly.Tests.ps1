# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

BeforeAll {
    . (Join-Path $PSScriptRoot '..\..\_common\TestHelpers.ps1')

    $script:RepoRoot = Get-OxiRepoRoot
    $script:ConstantsFile = Join-Path $script:RepoRoot 'constants.env'
    $script:SettingsTemplate = Join-Path $script:RepoRoot '.vscode\settings.template.jsonc'
}

Describe 'Pinned nightly toolchain' {
    It 'pins the same nightly in settings.template.jsonc as RUST_NIGHTLY in constants.env' {
        $constantsNightly = (Get-Content $script:ConstantsFile | Select-String '^RUST_NIGHTLY=(.+)$').Matches.Groups[1].Value
        $constantsNightly | Should -Match '^nightly-\d{4}-\d{2}-\d{2}$'

        $pins = [regex]::Matches((Get-Content $script:SettingsTemplate -Raw), 'nightly-\d{4}-\d{2}-\d{2}')
        $pins.Count | Should -BeGreaterThan 0 -Because 'the template pins rustfmt to a specific nightly'

        foreach ($pin in $pins) {
            $pin.Value | Should -Be $constantsNightly -Because 'scripts/update_rust_toolchain.ps1 must carry a RUST_NIGHTLY bump into the template'
        }
    }
}
