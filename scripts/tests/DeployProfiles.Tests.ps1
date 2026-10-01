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

    Context 'hidden files' {
        BeforeEach {
            # A dot file is hidden to PowerShell on Linux; on Windows it needs the Hidden attribute too.
            function Set-HiddenFile([string]$path, [string]$text) {
                New-Item -ItemType Directory -Force (Split-Path $path) | Out-Null
                Set-Content $path $text
                if ($IsWindows) { [IO.File]::SetAttributes($path, 'Hidden') }
            }
            $script:target = Join-Path $script:config 'profiles'
            New-Item -ItemType Directory (Join-Path $script:target 'dev') | Out-Null
            Copy-Item (Join-Path $script:source 'dev\profile.toml') (Join-Path $script:target 'dev\profile.toml')
        }

        It 'deploys a changed hidden nested file and says updated' {
            Set-HiddenFile (Join-Path $script:source 'dev\files\.rules') 'new'
            Set-HiddenFile (Join-Path $script:target 'dev\files\.rules') 'old'

            $out = & $script:deploy -Source $script:source -ConfigDir $script:config 6>&1 | Out-String

            $out | Should -Match 'dev: updated'
            Get-Content (Join-Path $script:target 'dev\files\.rules') -Force | Should -Be 'new'
        }

        It 'removes a hidden file the repo no longer has and says updated' {
            Set-HiddenFile (Join-Path $script:target 'dev\.stale') 'left over'

            $out = & $script:deploy -Source $script:source -ConfigDir $script:config 6>&1 | Out-String

            $out | Should -Match 'dev: updated'
            Test-Path (Join-Path $script:target 'dev\.stale') | Should -BeFalse
        }

        It 'says unchanged when the hidden files match too' {
            Set-HiddenFile (Join-Path $script:source 'dev\.rules') 'same'
            Set-HiddenFile (Join-Path $script:target 'dev\.rules') 'same'

            (& $script:deploy -Source $script:source -ConfigDir $script:config 6>&1 | Out-String) | Should -Match 'dev: unchanged'
        }
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

    It 'reads a quoted profiles_dir key, basic or literal, like sbxm' -ForEach @(
        @{ Name = 'basic'; Key = '"profiles_dir"' }
        @{ Name = 'literal'; Key = "'profiles_dir'" }
        @{ Name = 'escaped basic'; Key = '"profiles\u005fdir"' }
    ) {
        $custom = Join-Path $TestDrive ([guid]::NewGuid())
        Set-Content (Join-Path $script:config 'config.toml') "base_dir = 'E:\x'`n$Key = '$custom'"

        & $script:deploy -Source $script:source -ConfigDir $script:config 6>$null

        Test-Path (Join-Path $custom 'dev\profile.toml') | Should -BeTrue
        Test-Path (Join-Path $script:config 'profiles') | Should -BeFalse
    }

    It 'refuses a quoted or dotted key sbxm would refuse, before writing anything' -ForEach @(
        @{ Line = "`"profiles_dri`" = 'E:\elsewhere'" }
        @{ Line = "'profiles_dri' = 'E:\elsewhere'" }
        @{ Line = "profiles.dir = 'E:\elsewhere'" }
    ) {
        Set-Content (Join-Path $script:config 'config.toml') "base_dir = 'E:\x'`n$Line"

        { & $script:deploy -Source $script:source -ConfigDir $script:config 6>$null } | Should -Throw '*sbxm would refuse*'

        Test-Path (Join-Path $script:config 'profiles') | Should -BeFalse
    }

    It 'accepts a quoted or dotted form of the other known keys' {
        Set-Content (Join-Path $script:config 'config.toml') "`"base_dir`" = 'E:\x'`nresources.cpus = 4`nresources.memory = '8g'"

        & $script:deploy -Source $script:source -ConfigDir $script:config 6>$null

        Test-Path (Join-Path $script:config 'profiles\dev\profile.toml') | Should -BeTrue
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
