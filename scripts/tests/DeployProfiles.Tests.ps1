BeforeAll {
    $script:deploy = Join-Path $PSScriptRoot '..\deploy-profiles.ps1'
    $script:repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path

    # A stand-in for `sbxm config profiles-dir` that prints $path. What sbxm accepts as config is covered by
    # tests/config_profiles_dir.rs; these tests cover what the script does with the answer.
    function New-Stub([string]$path) { [scriptblock]::Create("'" + $path.Replace("'", "''") + "'") }
}

Describe 'deploy-profiles.ps1' {
    BeforeEach {
        $root = Join-Path $TestDrive ([guid]::NewGuid())
        $script:source = Join-Path $root 'repo-profiles'
        $script:target = Join-Path $root 'sbxm-profiles'
        New-Item -ItemType Directory (Join-Path $script:source 'dev'), (Join-Path $script:source 'other') | Out-Null
        Set-Content (Join-Path $script:source 'dev\profile.toml') 'description = "dev"'
        Set-Content (Join-Path $script:source 'other\profile.toml') 'description = "other"'
        $script:stub = New-Stub $script:target
    }

    It 'copies every profile into the folder sbxm reports' {
        & $script:deploy -Source $script:source -ProfilesDirCommand $script:stub 6>$null

        Get-Content (Join-Path $script:target 'dev\profile.toml') | Should -Be 'description = "dev"'
        Get-Content (Join-Path $script:target 'other\profile.toml') | Should -Be 'description = "other"'
    }

    It 'replaces a changed profile, drops files the repo no longer has and leaves other profiles alone' {
        New-Item -ItemType Directory (Join-Path $script:target 'dev'), (Join-Path $script:target 'mine') | Out-Null
        Set-Content (Join-Path $script:target 'dev\profile.toml') 'description = "old"'
        Set-Content (Join-Path $script:target 'dev\stale.txt') 'left over'
        Set-Content (Join-Path $script:target 'mine\profile.toml') 'description = "mine"'

        & $script:deploy -Source $script:source -ProfilesDirCommand $script:stub 6>$null

        Get-Content (Join-Path $script:target 'dev\profile.toml') | Should -Be 'description = "dev"'
        Test-Path (Join-Path $script:target 'dev\stale.txt') | Should -BeFalse
        Get-Content (Join-Path $script:target 'mine\profile.toml') | Should -Be 'description = "mine"'
    }

    It 'copies nested files, such as instruction files' {
        New-Item -ItemType Directory (Join-Path $script:source 'dev\files') | Out-Null
        Set-Content (Join-Path $script:source 'dev\files\notes.md') 'hello'

        & $script:deploy -Source $script:source -ProfilesDirCommand $script:stub 6>$null

        Get-Content (Join-Path $script:target 'dev\files\notes.md') | Should -Be 'hello'
    }

    It 'says which profiles are new, updated or unchanged' {
        New-Item -ItemType Directory (Join-Path $script:target 'other') | Out-Null
        Set-Content (Join-Path $script:target 'other\profile.toml') 'description = "other"'

        $out = & $script:deploy -Source $script:source -ProfilesDirCommand $script:stub 6>&1 | Out-String
        $again = & $script:deploy -Source $script:source -ProfilesDirCommand $script:stub 6>&1 | Out-String

        $out | Should -Match 'dev: new'
        $out | Should -Match 'other: unchanged'
        $again | Should -Match 'dev: unchanged'
        Set-Content (Join-Path $script:target 'dev\profile.toml') 'description = "changed"'
        (& $script:deploy -Source $script:source -ProfilesDirCommand $script:stub 6>&1 | Out-String) | Should -Match 'dev: updated'
    }

    It 'uses the folder sbxm prints exactly, spaces at the ends of the name included' {
        # Windows can't create a folder with a trailing space, so look at what the script hands to the copier.
        $log = Join-Path $TestDrive ([guid]::NewGuid())
        $recorder = [scriptblock]::Create("param(`$from, `$to) Set-Content -LiteralPath '$log' -Value `$to -NoNewline; throw 'stop here'")
        $spaced = "$($script:target) "

        { & $script:deploy -Source $script:source -ProfilesDirCommand (New-Stub $spaced) -Copier $recorder 6>$null } | Should -Throw '*stop here*'

        (Get-Content -LiteralPath $log -Raw).StartsWith("$spaced$([IO.Path]::DirectorySeparatorChar).deploy-") | Should -BeTrue
    }

    It 'treats a change of file name case as an update and keeps the source casing' {
        Set-Content (Join-Path $script:source 'dev\Rules.md') 'same text'
        New-Item -ItemType Directory (Join-Path $script:target 'dev') | Out-Null
        Copy-Item (Join-Path $script:source 'dev\profile.toml') (Join-Path $script:target 'dev\profile.toml')
        Set-Content (Join-Path $script:target 'dev\rules.md') 'same text'

        $out = & $script:deploy -Source $script:source -ProfilesDirCommand $script:stub 6>&1 | Out-String

        $out | Should -Match 'dev: updated'
        # -ccontains: Pester's Contain ignores case.
        $names = @(Get-ChildItem (Join-Path $script:target 'dev')).Name
        ($names -ccontains 'Rules.md') | Should -BeTrue
        ($names -ccontains 'rules.md') | Should -BeFalse
    }

    It 'deploys from and to folders whose names contain wildcard characters' {
        $odd = Join-Path $TestDrive 'sbxm [repo] x'
        $oddSource = Join-Path $odd 'profiles'
        $oddTarget = Join-Path $odd 'deployed [profiles]'
        New-Item -ItemType Directory (Join-Path $oddSource 'dev\files') | Out-Null
        Set-Content -LiteralPath (Join-Path $oddSource 'dev\profile.toml') 'description = "dev"'
        Set-Content -LiteralPath (Join-Path $oddSource 'dev\files\notes.md') 'hello'

        $first = & $script:deploy -Source $oddSource -ProfilesDirCommand (New-Stub $oddTarget) 6>&1 | Out-String
        $second = & $script:deploy -Source $oddSource -ProfilesDirCommand (New-Stub $oddTarget) 6>&1 | Out-String

        $first | Should -Match 'dev: new'
        Get-Content -LiteralPath (Join-Path $oddTarget 'dev\files\notes.md') | Should -Be 'hello'
        $second | Should -Match 'dev: unchanged'
    }

    It 'deploys an empty folder the repo has, since a profile can point at one' {
        New-Item -ItemType Directory (Join-Path $script:source 'dev\home') | Out-Null
        New-Item -ItemType Directory (Join-Path $script:target 'dev') | Out-Null
        Copy-Item (Join-Path $script:source 'dev\profile.toml') (Join-Path $script:target 'dev\profile.toml')

        $out = & $script:deploy -Source $script:source -ProfilesDirCommand $script:stub 6>&1 | Out-String

        $out | Should -Match 'dev: updated'
        Test-Path -LiteralPath (Join-Path $script:target 'dev\home') -PathType Container | Should -BeTrue
    }

    It 'removes an empty folder the repo no longer has' {
        New-Item -ItemType Directory (Join-Path $script:target 'dev\gone') | Out-Null
        Copy-Item (Join-Path $script:source 'dev\profile.toml') (Join-Path $script:target 'dev\profile.toml')

        $out = & $script:deploy -Source $script:source -ProfilesDirCommand $script:stub 6>&1 | Out-String

        $out | Should -Match 'dev: updated'
        Test-Path -LiteralPath (Join-Path $script:target 'dev\gone') | Should -BeFalse
    }

    It 'says unchanged when the empty folders match too' {
        New-Item -ItemType Directory (Join-Path $script:source 'dev\home'), (Join-Path $script:target 'dev\home') | Out-Null
        Copy-Item (Join-Path $script:source 'dev\profile.toml') (Join-Path $script:target 'dev\profile.toml')

        (& $script:deploy -Source $script:source -ProfilesDirCommand $script:stub 6>&1 | Out-String) | Should -Match 'dev: unchanged'
    }

    It 'ignores folders without a profile.toml' {
        New-Item -ItemType Directory (Join-Path $script:source 'not-a-profile') | Out-Null

        & $script:deploy -Source $script:source -ProfilesDirCommand $script:stub 6>$null

        Test-Path (Join-Path $script:target 'not-a-profile') | Should -BeFalse
    }

    It 'keeps the old profile intact and leaves no staging folder when the copy fails' {
        New-Item -ItemType Directory (Join-Path $script:target 'dev') | Out-Null
        Set-Content (Join-Path $script:target 'dev\profile.toml') 'description = "old"'
        $failing = { param($from, $to) throw 'injected copy failure' }

        { & $script:deploy -Source $script:source -ProfilesDirCommand $script:stub -Copier $failing 6>$null } | Should -Throw '*injected copy failure*'

        Get-Content (Join-Path $script:target 'dev\profile.toml') | Should -Be 'description = "old"'
        @(Get-ChildItem $script:target -Force).Name | Should -Be @('dev')
    }

    Context 'hidden files' {
        BeforeEach {
            # A dot file is hidden to PowerShell on Linux; on Windows it needs the Hidden attribute too.
            function Set-HiddenFile([string]$path, [string]$text) {
                New-Item -ItemType Directory -Force (Split-Path $path) | Out-Null
                Set-Content $path $text
                if ($IsWindows) { [IO.File]::SetAttributes($path, 'Hidden') }
            }
            New-Item -ItemType Directory (Join-Path $script:target 'dev') | Out-Null
            Copy-Item (Join-Path $script:source 'dev\profile.toml') (Join-Path $script:target 'dev\profile.toml')
        }

        It 'deploys a changed hidden nested file and says updated' {
            Set-HiddenFile (Join-Path $script:source 'dev\files\.rules') 'new'
            Set-HiddenFile (Join-Path $script:target 'dev\files\.rules') 'old'

            $out = & $script:deploy -Source $script:source -ProfilesDirCommand $script:stub 6>&1 | Out-String

            $out | Should -Match 'dev: updated'
            Get-Content (Join-Path $script:target 'dev\files\.rules') -Force | Should -Be 'new'
        }

        It 'removes a hidden file the repo no longer has and says updated' {
            Set-HiddenFile (Join-Path $script:target 'dev\.stale') 'left over'

            $out = & $script:deploy -Source $script:source -ProfilesDirCommand $script:stub 6>&1 | Out-String

            $out | Should -Match 'dev: updated'
            Test-Path (Join-Path $script:target 'dev\.stale') | Should -BeFalse
        }

        It 'says unchanged when the hidden files match too' {
            Set-HiddenFile (Join-Path $script:source 'dev\.rules') 'same'
            Set-HiddenFile (Join-Path $script:target 'dev\.rules') 'same'

            (& $script:deploy -Source $script:source -ProfilesDirCommand $script:stub 6>&1 | Out-String) | Should -Match 'dev: unchanged'
        }
    }

    Context 'asking sbxm for the folder' {
        It 'stops with sbxm message and writes nothing when sbxm refuses the config' {
            $refusing = [scriptblock]::Create(@'
pwsh -NoProfile -Command "[Console]::Error.WriteLine('unknown field profiles_dri'); exit 1"
'@)

            { & $script:deploy -Source $script:source -ProfilesDirCommand $refusing 6>$null } | Should -Throw '*unknown field profiles_dri*'

            Test-Path $script:target | Should -BeFalse
        }

        It 'stops when sbxm prints no folder' {
            { & $script:deploy -Source $script:source -ProfilesDirCommand { '' } 6>$null } | Should -Throw '*no folder*'

            Test-Path $script:target | Should -BeFalse
        }

        It 'says how to run it when sbxm cannot be started' {
            $missing = { this-command-does-not-exist-sbxm }

            { & $script:deploy -Source $script:source -ProfilesDirCommand $missing 6>$null } | Should -Throw '*-ProfilesDirCommand*'
        }

        It 'ignores progress lines that are not the folder, such as cargo output on stderr' {
            $noisy = [scriptblock]::Create(@"
pwsh -NoProfile -Command "[Console]::Error.WriteLine('   Compiling sbxm'); Write-Output '$($script:target.Replace("'", "''"))'"
"@)

            & $script:deploy -Source $script:source -ProfilesDirCommand $noisy 6>$null

            Test-Path (Join-Path $script:target 'dev\profile.toml') | Should -BeTrue
        }

        It 'hands -ConfigDir to sbxm as SBXM_CONFIG_DIR for the call only' {
            $before = $env:SBXM_CONFIG_DIR
            $echo = { $env:SBXM_CONFIG_DIR }
            $configDir = Join-Path $TestDrive ([guid]::NewGuid())

            & $script:deploy -Source $script:source -ConfigDir $configDir -ProfilesDirCommand $echo 6>$null

            Test-Path (Join-Path $configDir 'dev\profile.toml') | Should -BeTrue
            $env:SBXM_CONFIG_DIR | Should -Be $before
        }

        It 'asks the real sbxm, so its config rules apply' -Skip:(-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
            $config = Join-Path $TestDrive ([guid]::NewGuid())
            New-Item -ItemType Directory $config | Out-Null
            $custom = Join-Path $TestDrive ([guid]::NewGuid())
            Set-Content (Join-Path $config 'config.toml') "`"base_dir`" = 'base'`n`"profiles_dir`" = '$custom'`n[resources]`ncpus = 4`nmemory = '8g'"
            $real = [scriptblock]::Create("cargo run --quiet --manifest-path '$($script:repo)/Cargo.toml' -- config profiles-dir")

            & $script:deploy -Source $script:source -ConfigDir $config -ProfilesDirCommand $real 6>$null
            Test-Path (Join-Path $custom 'dev\profile.toml') | Should -BeTrue

            Set-Content (Join-Path $config 'config.toml') "base_dir = 'base'`nprofiles_dri = 'x'"
            $other = Join-Path $TestDrive ([guid]::NewGuid())
            { & $script:deploy -Source $script:source -ConfigDir $config -ProfilesDirCommand $real 6>$null } | Should -Throw '*profiles_dri*'
        }
    }

    It 'fails when the source folder is missing, before touching anything' {
        { & $script:deploy -Source (Join-Path $TestDrive 'nope') -ProfilesDirCommand $script:stub 6>$null } | Should -Throw '*not found*'
        Test-Path $script:target | Should -BeFalse
    }
}
