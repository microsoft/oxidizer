# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

BeforeAll {
    . (Join-Path $env:OXI_TEST_COMMON 'TestHelpers.ps1')
    . (Join-Path (Get-OxiRepoRoot) 'scripts\lib\release-flow.ps1')
}

Describe 'Parse-ReleaseTokens' {
    Context 'change-type keywords' {
        It 'parses breaking, nonbreaking, patch (case-insensitive) into canonical kebab-case values' {
            $parsed = Parse-ReleaseTokens -Tokens @('a@breaking', 'b@nonbreaking', 'c@patch', 'd@BREAKING', 'e@NonBreaking')

            $parsed.Count | Should -Be 5

            $parsed[0].Name | Should -Be 'a'
            $parsed[0].RequestedChangeType | Should -Be 'breaking'
            $parsed[0].RequestedTargetVersion | Should -BeNullOrEmpty
            $parsed[0].RawToken | Should -Be 'a@breaking'

            $parsed[1].RequestedChangeType | Should -Be 'non-breaking'
            $parsed[2].RequestedChangeType | Should -Be 'patch'
            $parsed[3].RequestedChangeType | Should -Be 'breaking'
            $parsed[4].RequestedChangeType | Should -Be 'non-breaking'
        }

        It 'preserves the original case of the package name' {
            $parsed = Parse-ReleaseTokens -Tokens @('MixedCase_Name@patch')
            $parsed[0].Name | Should -Be 'MixedCase_Name'
        }

        It 'records the raw token verbatim including whitespace before trim' {
            $parsed = Parse-ReleaseTokens -Tokens @('  spacey@patch  ')
            $parsed[0].Name | Should -Be 'spacey'
            $parsed[0].RawToken | Should -Be '  spacey@patch  '
        }
    }

    Context 'explicit version pins' {
        It 'treats 1.0.0 as an ordinary explicit pin (no special graduation handling)' {
            $parsed = Parse-ReleaseTokens -Tokens @('pkg@1.0.0')
            $parsed[0].RequestedChangeType | Should -BeNullOrEmpty
            $parsed[0].RequestedTargetVersion | Should -Be '1.0.0'
        }

        It 'accepts an arbitrary semver pin and leaves RequestedChangeType null' {
            $parsed = Parse-ReleaseTokens -Tokens @('pkg@1.2.3')
            $parsed[0].RequestedChangeType | Should -BeNullOrEmpty
            $parsed[0].RequestedTargetVersion | Should -Be '1.2.3'
        }

        It 'accepts large semver components' {
            $parsed = Parse-ReleaseTokens -Tokens @('pkg@42.987.65')
            $parsed[0].RequestedTargetVersion | Should -Be '42.987.65'
        }
    }

    Context 'duplicate rejection' {
        It 'rejects duplicate names (case-insensitive)' {
            { Parse-ReleaseTokens -Tokens @('foo@breaking', 'Foo@patch') } |
                Should -Throw -ExpectedMessage "*Duplicate package name 'Foo'*"
        }
    }

    Context 'malformed tokens' {
        It 'rejects an empty -Packages list' {
            { Parse-ReleaseTokens -Tokens @() } | Should -Throw -ExpectedMessage '*No packages to release*'
        }

        It 'rejects a null -Packages list' {
            { Parse-ReleaseTokens -Tokens $null } | Should -Throw -ExpectedMessage '*No packages to release*'
        }

        It 'rejects an empty / whitespace-only token' {
            { Parse-ReleaseTokens -Tokens @('') }   | Should -Throw -ExpectedMessage '*empty or whitespace-only token*'
            { Parse-ReleaseTokens -Tokens @('   ') }| Should -Throw -ExpectedMessage '*empty or whitespace-only token*'
        }

        It 'rejects tokens missing the @ separator' {
            { Parse-ReleaseTokens -Tokens @('pkg-breaking') } | Should -Throw -ExpectedMessage "*Malformed package token 'pkg-breaking'*"
        }

        It 'rejects tokens starting with @' {
            { Parse-ReleaseTokens -Tokens @('@breaking') } | Should -Throw -ExpectedMessage "*Malformed package token '@breaking'*"
        }

        It 'rejects tokens ending with @' {
            { Parse-ReleaseTokens -Tokens @('pkg@') } | Should -Throw -ExpectedMessage "*Malformed package token 'pkg@'*"
        }

        It 'rejects tokens with more than one @' {
            { Parse-ReleaseTokens -Tokens @('pkg@1.0.0@extra') } | Should -Throw -ExpectedMessage "*Malformed package token 'pkg@1.0.0@extra'*"
        }

        It 'rejects invalid change keywords' {
            { Parse-ReleaseTokens -Tokens @('pkg@major') }    | Should -Throw -ExpectedMessage "*Invalid change specifier 'major'*"
            { Parse-ReleaseTokens -Tokens @('pkg@feature') }  | Should -Throw -ExpectedMessage "*Invalid change specifier 'feature'*"
            { Parse-ReleaseTokens -Tokens @('pkg@1.0') }      | Should -Throw -ExpectedMessage "*Invalid change specifier '1.0'*"
            { Parse-ReleaseTokens -Tokens @('pkg@1') }        | Should -Throw -ExpectedMessage "*Invalid change specifier '1'*"
            { Parse-ReleaseTokens -Tokens @('pkg@1.2.3.4') }  | Should -Throw -ExpectedMessage "*Invalid change specifier '1.2.3.4'*"
            { Parse-ReleaseTokens -Tokens @('pkg@v1.2.3') }   | Should -Throw -ExpectedMessage "*Invalid change specifier 'v1.2.3'*"
        }

        It 'rejects invalid package names' {
            { Parse-ReleaseTokens -Tokens @('-bad@patch') }   | Should -Throw -ExpectedMessage "*Invalid package name '-bad'*"
            { Parse-ReleaseTokens -Tokens @('bad-@patch') }   | Should -Throw -ExpectedMessage "*Invalid package name 'bad-'*"
            { Parse-ReleaseTokens -Tokens @('has space@patch') } | Should -Throw -ExpectedMessage "*Invalid package name 'has space'*"
        }
    }
}
