BeforeAll {
    $script:deploy = Join-Path $PSScriptRoot '..\deploy-profiles.ps1'
}

Describe 'deploy-profiles.ps1' {
    BeforeEach {
        $root = Join-Path $TestDrive ([guid]::NewGuid())
        $script:source = Join-Path $root 'repo-profiles'
        $script:config = Join-Path $root 'config'
        New-Item -ItemType Directory (Join-Path $script:source 'dev'), (Join-Path $script:source 'other'), $script:config | Out-Null
        Set-Content (Join-Path $script:source 'dev\profile.toml') 'description = "dev"'
        Set-Content (Join-Path $script:source 'other\profile.toml') 'description = "other"'
    }

    It 'copies every profile into the config dir profiles folder when config.toml has no profiles_dir' {
        & $script:deploy -Source $script:source -ConfigDir $script:config 6>$null

        Get-Content (Join-Path $script:config 'profiles\dev\profile.toml') | Should -Be 'description = "dev"'
        Get-Content (Join-Path $script:config 'profiles\other\profile.toml') | Should -Be 'description = "other"'
    }

    It 'uses the profiles_dir that config.toml names' {
        $custom = Join-Path $TestDrive ([guid]::NewGuid())
        Set-Content (Join-Path $script:config 'config.toml') "base_dir = 'E:\x'`nprofiles_dir = '$custom'"

        & $script:deploy -Source $script:source -ConfigDir $script:config 6>$null

        Test-Path (Join-Path $custom 'dev\profile.toml') | Should -BeTrue
        Test-Path (Join-Path $script:config 'profiles') | Should -BeFalse
    }

    It 'replaces a changed profile, drops files the repo no longer has and leaves other profiles alone' {
        $target = Join-Path $script:config 'profiles'
        New-Item -ItemType Directory (Join-Path $target 'dev'), (Join-Path $target 'mine') | Out-Null
        Set-Content (Join-Path $target 'dev\profile.toml') 'description = "old"'
        Set-Content (Join-Path $target 'dev\stale.txt') 'left over'
        Set-Content (Join-Path $target 'mine\profile.toml') 'description = "mine"'

        & $script:deploy -Source $script:source -ConfigDir $script:config 6>$null

        Get-Content (Join-Path $target 'dev\profile.toml') | Should -Be 'description = "dev"'
        Test-Path (Join-Path $target 'dev\stale.txt') | Should -BeFalse
        Get-Content (Join-Path $target 'mine\profile.toml') | Should -Be 'description = "mine"'
    }

    It 'copies nested files, such as instruction files' {
        New-Item -ItemType Directory (Join-Path $script:source 'dev\files') | Out-Null
        Set-Content (Join-Path $script:source 'dev\files\notes.md') 'hello'

        & $script:deploy -Source $script:source -ConfigDir $script:config 6>$null

        Get-Content (Join-Path $script:config 'profiles\dev\files\notes.md') | Should -Be 'hello'
    }

    It 'says which profiles are new, updated or unchanged' {
        $target = Join-Path $script:config 'profiles'
        New-Item -ItemType Directory (Join-Path $target 'other') | Out-Null
        Set-Content (Join-Path $target 'other\profile.toml') 'description = "other"'

        $out = & $script:deploy -Source $script:source -ConfigDir $script:config 6>&1 | Out-String
        $again = & $script:deploy -Source $script:source -ConfigDir $script:config 6>&1 | Out-String

        $out | Should -Match 'dev: new'
        $out | Should -Match 'other: unchanged'
        $again | Should -Match 'dev: unchanged'
        Set-Content (Join-Path $target 'dev\profile.toml') 'description = "changed"'
        (& $script:deploy -Source $script:source -ConfigDir $script:config 6>&1 | Out-String) | Should -Match 'dev: updated'
    }

    It 'ignores folders without a profile.toml' {
        New-Item -ItemType Directory (Join-Path $script:source 'not-a-profile') | Out-Null

        & $script:deploy -Source $script:source -ConfigDir $script:config 6>$null

        Test-Path (Join-Path $script:config 'profiles\not-a-profile') | Should -BeFalse
    }

    It 'decodes TOML escapes in a double-quoted profiles_dir, as sbxm does' {
        $custom = Join-Path $TestDrive ([guid]::NewGuid())
        $escaped = ($custom -replace '\\', '\\') -replace 't', '\u0074'
        Set-Content (Join-Path $script:config 'config.toml') "base_dir = 'E:\x'`nprofiles_dir = `"$escaped`""

        & $script:deploy -Source $script:source -ConfigDir $script:config 6>$null

        Test-Path (Join-Path $custom 'dev\profile.toml') | Should -BeTrue
    }

    It 'refuses a config.toml key sbxm would refuse, before writing anything' {
        Set-Content (Join-Path $script:config 'config.toml') "base_dir = 'E:\x'`nprofiles_dri = 'E:\elsewhere'"

        { & $script:deploy -Source $script:source -ConfigDir $script:config 6>$null } | Should -Throw '*profiles_dri*'

        Test-Path (Join-Path $script:config 'profiles') | Should -BeFalse
    }

    It 'refuses a profiles_dir it cannot read as a quoted string' {
        Set-Content (Join-Path $script:config 'config.toml') "base_dir = 'E:\x'`nprofiles_dir = 5"

        { & $script:deploy -Source $script:source -ConfigDir $script:config 6>$null } | Should -Throw '*profiles_dir*'

        Test-Path (Join-Path $script:config 'profiles') | Should -BeFalse
    }

    It 'keeps the other keys and the resources table working' {
        Set-Content (Join-Path $script:config 'config.toml') "base_dir = 'E:\x'`ndefault_profile = 'dev'`nmin_sbx_version = '0.43.0'`n[resources]`ncpus = 4`nmemory = '8g'"

        & $script:deploy -Source $script:source -ConfigDir $script:config 6>$null

        Test-Path (Join-Path $script:config 'profiles\dev\profile.toml') | Should -BeTrue
    }

    It 'keeps the old profile intact and leaves no staging folder when the copy fails' {
        $target = Join-Path $script:config 'profiles'
        New-Item -ItemType Directory (Join-Path $target 'dev') | Out-Null
        Set-Content (Join-Path $target 'dev\profile.toml') 'description = "old"'
        $failing = { param($from, $to) throw 'injected copy failure' }

        { & $script:deploy -Source $script:source -ConfigDir $script:config -Copier $failing 6>$null } | Should -Throw '*injected copy failure*'

        Get-Content (Join-Path $target 'dev\profile.toml') | Should -Be 'description = "old"'
        @(Get-ChildItem $target -Force).Name | Should -Be @('dev')
    }

    It 'fails when the source folder is missing, before touching anything' {
        { & $script:deploy -Source (Join-Path $TestDrive 'nope') -ConfigDir $script:config 6>$null } | Should -Throw '*not found*'
        Test-Path (Join-Path $script:config 'profiles') | Should -BeFalse
    }
}
