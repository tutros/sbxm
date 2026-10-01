# The functions behind file-review-issues.ps1 (decision 134): parse a review file from sdlc/reviews/, render one issue
# per finding, and file them from a host where `gh` works. Nothing here is specific to this host, so M2b can move
# the parser and renderer into sbxm.
$ErrorActionPreference = 'Stop'

$sectionLabels = @{ 'must fix' = 'must-fix'; 'should fix' = 'should-fix'; 'question' = 'question'; 'questions' = 'question' }
$fieldAliases = @{
    'smallest fix' = 'fix'; 'recommendation' = 'fix'; 'options and recommendation' = 'fix'
}
$dashes = '—|–|-'

# Text to a review: Scope, Issues line, findings and what isn't filed.
function ConvertFrom-ReviewFile {
    param([Parameter(Mandatory)][string]$Text)
    $lines = $Text -replace "`r`n", "`n" -split "`n"
    $review = [ordered]@{
        Findings = [System.Collections.Generic.List[object]]::new()
        NotFiled = [ordered]@{ Sections = 0; Nits = 0 }
        HeadSha = $null; Range = $null; Scope = $null; ScopeLine = $null; IssuesLine = $null; IssuesLineCount = 0
    }
    $section = $null     # @{ Label; Findings = count }
    $finding = $null
    $sections = [System.Collections.Generic.List[object]]::new()
    for ($i = 0; $i -lt $lines.Count; $i++) {
        $line = $lines[$i]
        if ($line -match '^Scope:\s*(.*)$' -and -not $review.Scope) {
            $review.Scope = $Matches[1]
            $review.ScopeLine = $i + 1
            $sha = [regex]::Match($Matches[1], '\b[0-9a-f]{7,40}\.\.([0-9a-f]{7,40})\b')
            if ($sha.Success) { $review.HeadSha = $sha.Groups[1].Value; $review.Range = $sha.Value }
        }
        elseif ($line -match '^Issues:') {
            $review.IssuesLineCount++
            if (-not $review.IssuesLine) { $review.IssuesLine = $line }
        }
        if ($line -match '^##\s+(.+?)\s*$' -and $line -notmatch '^###') {
            if ($finding) { $review.Findings.Add([pscustomobject](Complete-Finding $finding)); $finding = $null }
            $name = ($Matches[1] -replace ':+\s*$', '').Trim().ToLowerInvariant()
            $section = @{ Label = $sectionLabels[$name]; Findings = 0; Nits = 0 }
            $sections.Add($section)
            continue
        }
        if ($section -and $section.Label -and $line -match "^###\s+(\S+?)(?:\s+(?:$dashes)\s+|:\s+|\s+:\s+)(.+?)\s*$") {
            if ($finding) { $review.Findings.Add([pscustomobject](Complete-Finding $finding)); $finding = $null }
            $section.Findings++
            $finding = @{
                Id = $Matches[1]; Title = $Matches[2]; Label = $section.Label; StartLine = $i + 1
                Body = [System.Collections.Generic.List[string]]::new()
            }
            continue
        }
        if ($finding) { $finding.Body.Add($line) }
        elseif ($section -and -not $section.Label -and $line -match '^\s*[-*]\s+Nit:') { $section.Nits++ }
    }
    if ($finding) { $review.Findings.Add([pscustomobject](Complete-Finding $finding)) }
    foreach ($s in $sections) {
        if ($s.Findings -eq 0) { $review.NotFiled.Sections++ }
        $review.NotFiled.Nits += $s.Nits
    }
    [pscustomobject]$review
}

# Splits a finding's body into fields and works out which required ones are missing.
function Complete-Finding([hashtable]$finding) {
    $fields = [ordered]@{}
    $criteria = [System.Collections.Generic.List[string]]::new()
    $current = $null
    $values = @{}
    foreach ($line in $finding.Body) {
        if ($line -match '^\*\*([^*:]+):\*\*\s*(.*)$') {
            $name = $Matches[1].Trim().ToLowerInvariant()
            if ($fieldAliases.ContainsKey($name)) { $name = $fieldAliases[$name] }
            $current = $name
            $values[$current] = [System.Collections.Generic.List[string]]::new()
            if ($Matches[2]) { $values[$current].Add($Matches[2]) }
            continue
        }
        if ($current) { $values[$current].Add($line) }
    }
    foreach ($name in $values.Keys) {
        $text = ($values[$name] -join "`n").Trim()
        if ($name -eq 'acceptance criteria') {
            foreach ($item in $values[$name]) {
                if ($item -match '^\s*[-*]\s+\[[ xX]\]\s*(.+?)\s*$') { $criteria.Add($Matches[1]) }
            }
        }
        elseif ($text) { $fields[$name] = $text }
    }
    $required = 'where', 'what happens', 'why it matters', 'fix'
    $finding.Fields = $fields
    $finding.Criteria = $criteria.ToArray()
    $finding.Missing = @($required | Where-Object { -not $fields.Contains($_) })
    $finding.EndLine = $finding.StartLine + $finding.Body.Count
    $finding.Remove('Body')
    $finding
}

$secretPatterns = [ordered]@{
    'a GitHub token'                = '\b(?:gh[pos]_[A-Za-z0-9]{16,}|github_pat_[A-Za-z0-9_]{16,})'
    'an sk- key'                    = '\bsk-[A-Za-z0-9_-]{20,}'
    'an AWS key'                    = '\bAKIA[0-9A-Z]{16}\b'
    'a bearer token'                = 'Bearer\s+[A-Za-z0-9._~+/=-]{20,}'
    'a private key'                 = '-----BEGIN [A-Z ]*PRIVATE KEY-----'
    'a password or token assignment' = '(?i)(?<![A-Za-z])(?:password|passwd|token|secret)\s*=(?!=)\s*(?:"[^"]+"|''[^'']+''|[^\s"'']+)'
}

# The kind of secret a piece of text matches, or nothing. Used both per line (Find-Secrets) and for text that
# isn't tied to a finding's line range, such as the Scope line's fallback text or the review file name.
function Find-SecretKind([string]$Text) {
    foreach ($kind in $secretPatterns.Keys) {
        if ($Text -match $secretPatterns[$kind]) { return $kind }
    }
    $null
}

# Lines of a finding that look like a secret: id, line number in the review file and what it looks like, never
# the value. The caller refuses the finding.
function Find-Secrets {
    param([Parameter(Mandatory)][string]$Text, [Parameter(Mandatory)]$Finding)
    $lines = $Text -replace "`r`n", "`n" -split "`n"
    $last = [Math]::Min($Finding.EndLine, $lines.Count)
    for ($n = $Finding.StartLine; $n -le $last; $n++) {
        $kind = Find-SecretKind $lines[$n - 1]
        if ($kind) {
            [pscustomobject]@{ Id = $Finding.Id; Line = $n; Kind = $kind }
        }
    }
}

# Personal paths become ~ (unless -KeepPaths) and e-mail addresses are flagged, each with a warning.
function Protect-Text {
    param([Parameter(Mandatory, Position = 0)][AllowEmptyString()][string]$Text, [Parameter(Mandatory)][string]$Id, [switch]$KeepPaths)
    if (-not $KeepPaths) {
        # The profile folder may hold spaces ("Mary Jane Watson Parker"): when a separator, quote or backtick ends
        # it, the whole segment goes, however many words it has; otherwise only the first word does, so prose after
        # a bare path is left alone. A newline always ends it.
        $replacements = @{
            '(?i)\b[A-Z]:\\Users\\(?:[^\\\s`''"]+(?: [^\\\s`''"]+)*(?=[\\`''"])|[^\\\s`''"]+)' = '~'
            '/(?:home|Users)/(?:[^/\s`''"]+(?: [^/\s`''"]+)*(?=[/`''"])|[^/\s`''"]+)' = '~'
        }
        foreach ($pattern in $replacements.Keys) {
            $hits = [regex]::Matches($Text, $pattern).Count
            if ($hits) {
                Write-Warning "${Id}: replaced $hits personal path(s) with ~; use -KeepPaths to keep them"
                $Text = [regex]::Replace($Text, $pattern, $replacements[$pattern])
            }
        }
    }
    if ($Text -match '[\w.+-]+@[\w-]+(\.[\w-]+)+') { Write-Warning "${Id}: contains an e-mail address (left as is)" }
    $Text
}

# A copy of the finding with every text protected.
function Protect-Finding {
    param([Parameter(Mandatory)]$Finding, [switch]$KeepPaths)
    $copy = $Finding.PSObject.Copy()
    $copy.Title = Protect-Text $Finding.Title -Id $Finding.Id -KeepPaths:$KeepPaths
    $fields = [ordered]@{}
    foreach ($name in $Finding.Fields.Keys) { $fields[$name] = Protect-Text $Finding.Fields[$name] -Id $Finding.Id -KeepPaths:$KeepPaths }
    $copy.Fields = $fields
    $copy.Criteria = @($Finding.Criteria | ForEach-Object { Protect-Text $_ -Id $Finding.Id -KeepPaths:$KeepPaths })
    $copy
}

# A full sha stays as it is, a short one is resolved by git; nothing in, nothing out.
function Resolve-HeadSha([string]$Sha) {
    if (-not $Sha) { return $null }
    if ($Sha -match '^[0-9a-f]{40}$') { return $Sha }
    $full = git rev-parse --verify "$Sha^{commit}"
    if ($LASTEXITCODE -ne 0 -or -not $full) {
        Write-Warning "git can't resolve the reviewed commit $Sha here; Where links stay plain text"
        return $null
    }
    "$full".Trim()
}

# `src/a.rs:10-20` or `justfile:76` becomes a permalink at the reviewed commit. Plain words are left alone.
function ConvertTo-WhereLinks {
    param([string]$Text, [string]$Repo, [string]$Sha)
    $known = 'rs|toml|md|ps1|psm1|json|ya?ml|lock|txt|sh|py|js|ts'
    # Extensionless names count only when they are a well-known file name and carry a line suffix, so words such
    # as "step:3" or "ratio 3:1" stay text.
    $bare = '(?:justfile|Dockerfile|Containerfile|Makefile|Rakefile|Gemfile|Procfile|Brewfile|Vagrantfile|Jenkinsfile|LICENSE)(?=:\d)'
    $pattern = '(?<![\w/.:-])(`?)((?:[\w.-]+/)*(?:[\w.-]+\.\w+|' + $bare + '))(?::(\d+)(?:-(\d+))?)?(`?)'
    if (-not $Sha) {
        if ($Text -match $pattern) { Write-Warning 'no commit sha found in the Scope line; Where links stay plain text' }
        return $Text
    }
    [regex]::Replace($Text, $pattern, {
            param($m)
            $path = $m.Groups[2].Value
            $from = $m.Groups[3].Value
            $to = $m.Groups[4].Value
            if (-not ($path -match "\.($known)$" -or $path.Contains('/') -or $from)) { return $m.Value }
            $label = $path + $(if ($from) { ":$from" }) + $(if ($to) { "-$to" })
            $anchor = if ($to) { "#L$from-L$to" } elseif ($from) { "#L$from" } else { '' }
            $url = "https://github.com/$Repo/blob/$Sha/$path$anchor"
            if ($m.Groups[1].Value) { "[``$label``]($url)" } else { "[$label]($url)" }
        })
}

# Finding ids in a Depends on / Related text become issue numbers where they are known.
function Convert-FindingIds([string]$Text, [hashtable]$IdMap) {
    foreach ($id in $IdMap.Keys) {
        $Text = $Text -replace "(?<![\w-])$([regex]::Escape($id))(?![\w-])", "#$($IdMap[$id])"
    }
    $Text
}

# An existing issue's body with finding ids rewritten to #n inside its `Depends on` and `Related` fields, and
# nothing else, so hand edits elsewhere in the body stay. A field runs to the next bold field, comment or blank line.
function Update-LinkFields([string]$Body, [hashtable]$IdMap) {
    $pattern = '(?m)^\*\*(?:Depends on|Related):\*\*[^\r\n]*(?:\r?\n(?!\*\*|<!--|\r?$)[^\r\n]*)*'
    [regex]::Replace($Body, $pattern, { param($m) Convert-FindingIds $m.Value $IdMap })
}

function Format-Field([string]$Label, [string]$Value) {
    if ($Value.Contains("`n")) { "**${Label}:**`n$Value" } else { "**${Label}:** $Value" }
}

# The standard criteria from the skill's template, each with a pattern that tells whether a finding has it already.
$standardChecklist = @(
    @{ Pattern = 'fails before the fix'; Text = 'A test covering it fails before the fix and passes after (name it, or say which file it goes in)' }
    @{ Pattern = 'cargo fmt'; Text = '`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` pass' }
    @{ Pattern = 'Docs updated'; Text = 'Docs updated where behavior users see changed (`README.md`), or "no user-visible change"' }
)
$missingCriterion = '<the specific check is missing from the review: add one before working this issue>'

# The criteria an issue gets: the finding's own, plus the standard ones it lacks. A must-fix or should-fix with none
# is an error unless -StandardCriteria says to file it with only the standard ones. Questions get none.
function Get-AcceptanceCriteria {
    param([Parameter(Mandatory)]$Finding, [switch]$StandardCriteria)
    if ($Finding.Label -eq 'question') { return [pscustomobject]@{ Items = @(); Error = $null } }
    $own = @($Finding.Criteria)
    if ($own.Count -eq 0 -and -not $StandardCriteria) {
        return [pscustomobject]@{
            Items = @()
            Error = "$($Finding.Id) has no acceptance criteria; add them to the review file, or rerun with -StandardCriteria to file it with only the standard ones"
        }
    }
    $items = [System.Collections.Generic.List[string]]::new()
    if ($own.Count -eq 0) { $items.Add($missingCriterion) }
    foreach ($item in $own) { $items.Add($item) }
    foreach ($std in $standardChecklist) {
        if (-not ($own | Where-Object { $_ -match $std.Pattern })) { $items.Add($std.Text) }
    }
    [pscustomobject]@{ Items = $items.ToArray(); Error = $null }
}

# The issue text for one finding: the skill's template in order, the marker on the last line.
function New-IssueBody {
    param(
        [Parameter(Mandatory)]$Review, [Parameter(Mandatory)]$Finding, [Parameter(Mandatory)][string]$Repo,
        [Parameter(Mandatory)][string]$ReviewName, [string]$HeadSha, [hashtable]$IdMap = @{},
        [switch]$StandardCriteria
    )
    $criteria = Get-AcceptanceCriteria $Finding -StandardCriteria:$StandardCriteria
    if ($criteria.Error) { throw $criteria.Error }
    $f = $Finding.Fields
    $isQuestion = $Finding.Label -eq 'question'
    $depends = if ($f.Contains('depends on')) { $f['depends on'] } else { 'none known' }
    $pointsBack = @($Review.Findings | Where-Object {
            $_.Id -ne $Finding.Id -and $_.Fields.Contains('depends on') -and
            $_.Fields['depends on'] -match "(?<![\w-])$([regex]::Escape($Finding.Id))(?![\w-])"
        } | ForEach-Object { "$($_.Id) depends on this one" })
    $related = (@(if ($f.Contains('related')) { $f['related'] }) + $pointsBack) -join '; '

    $parts = [System.Collections.Generic.List[string]]::new()
    $parts.Add((Format-Field 'Where' (ConvertTo-WhereLinks $f['where'] $Repo $HeadSha)))
    $parts.Add((Format-Field 'What happens' $f['what happens']))
    $parts.Add((Format-Field 'Why it matters' $f['why it matters']))
    $parts.Add((Format-Field $(if ($isQuestion) { 'Options and recommendation' } else { 'Fix' }) $f['fix']))
    $parts.Add((Format-Field 'Depends on' (Convert-FindingIds $depends $IdMap)))
    if ($related) { $parts.Add((Format-Field 'Related' (Convert-FindingIds $related $IdMap))) }
    if (-not $isQuestion) {
        $parts.Add("**Acceptance criteria:**`n" + (($criteria.Items | ForEach-Object { "- [ ] $_" }) -join "`n"))
    }
    $range = if ($Review.Range) { $Review.Range } else { $Review.Scope }
    $parts.Add("**Review:** ``sdlc/reviews/$ReviewName``, finding $($Finding.Id), reviewed commits ``$range``")
    $parts.Add("<!-- review-finding: $ReviewName#$($Finding.Id) -->")
    ($parts -join "`n") + "`n"
}

function ConvertFrom-GhJson($Raw) { ($Raw -join "`n") | ConvertFrom-Json }

# Checks what the skill's section 8 asks for before anything is written. Returns the repo name, or throws one line
# that says what to fix.
function Test-ReviewAccess {
    param([string]$Repo, [string[]]$Labels)
    if ($Repo) { $view = gh repo view $Repo --json nameWithOwner }
    else {
        $remote = git remote get-url origin
        if ($LASTEXITCODE -ne 0 -or "$remote" -notmatch 'github\.com[:/]') {
            throw "origin isn't a GitHub repo here ($remote); run this from a checkout whose origin is on GitHub, or pass -Repo <owner/name>"
        }
        $view = gh repo view --json nameWithOwner
    }
    if ($LASTEXITCODE -ne 0) { throw "gh can't read the repo; check the name and run ``gh auth status``" }
    $name = (ConvertFrom-GhJson $view).nameWithOwner
    $auth = gh auth status 2>&1 | Out-String
    if ($LASTEXITCODE -ne 0) { throw "gh isn't logged in; run ``gh auth login`` and rerun" }
    if ($auth -match 'Token scopes:' -and $auth -notmatch "'repo'") {
        throw "the gh token lacks the repo scope; run ``gh auth refresh -s repo`` and rerun"
    }
    $existing = @((ConvertFrom-GhJson (gh label list --repo $name --json name --limit 200)).name)
    foreach ($label in $Labels) {
        if ($existing -notcontains $label) { throw "label '$label' doesn't exist in $name; create it (this script creates no labels)" }
    }
    $name
}

$issueListLimit = 1000

# Findings in an order where each comes after the ones it depends on (file order otherwise; a cycle keeps file order).
function Get-FilingOrder([object[]]$Findings) {
    $ordered = [System.Collections.Generic.List[object]]::new()
    $left = [System.Collections.Generic.List[object]]($Findings)
    while ($left.Count) {
        $ready = $left | Where-Object {
            $me = $_
            -not ($left | Where-Object {
                    $_.Id -ne $me.Id -and $me.Fields.Contains('depends on') -and
                    $me.Fields['depends on'] -match "(?<![\w-])$([regex]::Escape($_.Id))(?![\w-])"
                })
        } | Select-Object -First 1
        if (-not $ready) { $ready = $left[0] }
        $ordered.Add($ready)
        $left.Remove($ready) | Out-Null
    }
    $ordered
}

# Runs a gh command that takes --body-file with the text in a temp file, and deletes the file afterwards. Returns what
# the command printed; throws if it failed.
function Invoke-GhWithBody([scriptblock]$Command, [string]$Body) {
    $file = New-TemporaryFile
    try {
        [IO.File]::WriteAllText($file.FullName, $Body, [Text.UTF8Encoding]::new($false))
        $out = & $Command $file.FullName
        if ($LASTEXITCODE -ne 0) { throw "gh failed (exit code $LASTEXITCODE)" }
        $out
    }
    finally { Remove-Item -LiteralPath $file.FullName -Force -ErrorAction SilentlyContinue }
}

# Replaces the first `Issues:` line and nothing else: the file's line endings and byte order mark stay.
function Set-IssuesLine([string]$Path, [string]$Line) {
    $bytes = [IO.File]::ReadAllBytes($Path)
    $bom = $bytes.Length -ge 3 -and $bytes[0] -eq 0xEF -and $bytes[1] -eq 0xBB -and $bytes[2] -eq 0xBF
    $skip = if ($bom) { 3 } else { 0 }
    $text = [Text.UTF8Encoding]::new($false).GetString($bytes, $skip, $bytes.Length - $skip)
    $pattern = [regex]::new('(?m)^Issues:[^\r\n]*')
    if (-not $pattern.IsMatch($text)) {
        Write-Warning "$(Split-Path -Leaf $Path) has no 'Issues:' line to update; add one so the issue numbers are recorded"
        return
    }
    $new = $pattern.Replace($text, { param($m) $Line }, 1)
    $out = [Text.UTF8Encoding]::new($false).GetBytes($new)
    if ($bom) { $out = [byte[]](0xEF, 0xBB, 0xBF) + $out }
    [IO.File]::WriteAllBytes($Path, $out)
}

# Whether the review file can be written to right now, checked before the first `gh issue create` so a file a
# rerun can't update is caught up front instead of after issues are already published.
function Test-ReviewFileWritable([string]$Path) {
    try {
        $stream = [IO.File]::OpenWrite($Path)
        $stream.Close()
        $true
    }
    catch { $false }
}

function Get-Plural([int]$n, [string]$word) { if ($n -eq 1) { "$n $word" } else { "$n ${word}s" } }

# `Issues: S-1 #33, S-2 pending`: a number where the finding has an issue, `Unknown` where it would get one.
function Format-IssuesLine {
    param([object[]]$Findings, [hashtable]$Numbers, [string[]]$WouldFile = @(), [string]$Unknown = '#?')
    'Issues: ' + (($Findings | ForEach-Object {
                if ($Numbers.ContainsKey($_.Id)) { "$($_.Id) #$($Numbers[$_.Id])" }
                elseif ($WouldFile -contains $_.Id) { "$($_.Id) $Unknown" }
                else { "$($_.Id) pending" }
            }) -join ', ')
}

# Files (or, without -Create, only shows) the issues for a review file's findings. Returns the exit code: 0 done,
# 1 nothing filed. Everything it prints goes to the information stream.
function Invoke-ReviewFiling {
    param(
        [Parameter(Mandatory)][string]$Review, [string]$Repo, [switch]$Create, [switch]$StandardCriteria,
        [switch]$KeepPaths, [string[]]$Only
    )
    if (-not (Test-Path -LiteralPath $Review -PathType Leaf)) {
        Write-Host "review file $Review not found; give the path of a file in sdlc/reviews/"
        return 1
    }
    $resolvedPath = (Resolve-Path -LiteralPath $Review).Path
    $text = [IO.File]::ReadAllText($resolvedPath)
    $reviewName = Split-Path -Leaf $Review
    $parsed = ConvertFrom-ReviewFile $text

    # An id goes into titles, bodies, markers and the Issues line, so it can't be a way to carry a path or a secret
    # into an issue. Checked before any gh call, for every finding, not only the selected ones.
    $badIds = @($parsed.Findings | Where-Object { $_.Id -notmatch '^[A-Za-z0-9._-]+$' })
    if ($badIds) {
        foreach ($bad in $badIds) {
            Write-Host "refused: finding id $($bad.Id) (line $($bad.StartLine) of $reviewName) has characters outside A-Za-z0-9._-; rename it in the review file"
        }
        Write-Host "nothing was filed or changed: fix the finding ids in $reviewName first"
        return 1
    }

    # Checked before Test-ReviewAccess or any gh call: a duplicate id would otherwise let two blocks both reach
    # `gh issue create`, with the second overwriting the first's number in the write-back.
    $dupes = @($parsed.Findings | Group-Object Id | Where-Object Count -gt 1)
    if ($dupes) {
        foreach ($dup in $dupes) {
            $lines = ($dup.Group | ForEach-Object { $_.StartLine }) -join ' and '
            Write-Host "refused: $($dup.Name) is used by more than one finding in $reviewName, at lines $lines"
        }
        Write-Host "nothing was filed or changed: make finding ids unique in $reviewName first"
        return 1
    }

    # Also checked up front: the write-back needs exactly one 'Issues:' line to replace, and (with -Create) a
    # file it can actually write to, so a malformed or read-only review is never noticed only after issues exist.
    if ($parsed.IssuesLineCount -ne 1) {
        $what = if ($parsed.IssuesLineCount -eq 0) { 'has no' } else { "has $($parsed.IssuesLineCount)" }
        Write-Host "$reviewName $what 'Issues:' line(s); keep exactly one so issue numbers can be written back"
        return 1
    }
    if ($Create -and -not (Test-ReviewFileWritable $resolvedPath)) {
        Write-Host "can't update $reviewName to record issue numbers; check its file permissions and rerun"
        return 1
    }

    $selected = @($parsed.Findings)
    if ($Only) {
        $unknown = @($Only | Where-Object { $parsed.Findings.Id -notcontains $_ })
        if ($unknown) {
            Write-Host "-Only names $($unknown -join ', '), which isn't in $reviewName; its findings are $($parsed.Findings.Id -join ', ')"
            return 1
        }
        $selected = @($selected | Where-Object { $Only -contains $_.Id })
    }
    if (-not $selected) {
        Write-Host "no findings found in $reviewName; check its headings against the review file format in the code review skill"
        return 1
    }
    try { $repoName = Test-ReviewAccess -Repo $Repo -Labels @($selected.Label | Sort-Object -Unique) }
    catch {
        Write-Host "$($_.Exception.Message)`nnothing was filed or changed; the review stays marked 'Issues: pending (no GitHub access from this host)'"
        return 1
    }

    $headSha = Resolve-HeadSha $parsed.HeadSha
    $problems = [System.Collections.Generic.List[string]]::new()

    # The review file name and, when there is no commit sha, the Scope line's fallback text both go into every
    # issue body (the "Review:" line, and the review file name into the marker too), so they get the same
    # refusal and protection as a finding's own fields, before any of it is written anywhere.
    $nameSecret = Find-SecretKind $reviewName
    if ($nameSecret) { $problems.Add("the review file name looks like $nameSecret; rename the file") }
    $postedReviewName = if ($nameSecret) { $reviewName } else { Protect-Text $reviewName -Id 'the review file name' -KeepPaths:$KeepPaths }

    $scopeFallback = $null
    if (-not $parsed.Range) {
        $scopeSecret = Find-SecretKind $parsed.Scope
        if ($scopeSecret) { $problems.Add("Scope line $($parsed.ScopeLine) looks like $scopeSecret; remove it from the review file") }
        else { $scopeFallback = Protect-Text $parsed.Scope -Id 'Scope' -KeepPaths:$KeepPaths }
    }

    $prepared = [System.Collections.Generic.List[object]]::new()
    foreach ($finding in $selected) {
        $before = $problems.Count
        if ($finding.Missing.Count) { $problems.Add("$($finding.Id) is missing: $($finding.Missing -join ', ')") }
        $criteria = Get-AcceptanceCriteria $finding -StandardCriteria:$StandardCriteria
        if ($criteria.Error) { $problems.Add($criteria.Error) }
        foreach ($secret in @(Find-Secrets -Text $text -Finding $finding)) {
            $problems.Add("$($secret.Id) line $($secret.Line) looks like $($secret.Kind); remove it from the review file")
        }
        if ($problems.Count -eq $before) { $prepared.Add((Protect-Finding $finding -KeepPaths:$KeepPaths)) }
    }
    if ($problems.Count) {
        $problems | ForEach-Object { Write-Host "refused: $_" }
        $verb = if ($Create) { 'nothing was filed' } else { 'nothing would be filed' }
        Write-Host "${verb}: $(Get-Plural $problems.Count 'problem') to fix in $reviewName first"
        return 1
    }

    # What exists already: a finding whose marker is in an issue body keeps that number (idempotency).
    try {
        $raw = gh issue list --repo $repoName --state all --limit $issueListLimit --json number,title,body
        if ($LASTEXITCODE -ne 0) { throw "gh failed (exit code $LASTEXITCODE)" }
        $listed = @(ConvertFrom-GhJson $raw)
    }
    catch {
        Write-Host "can't read the issue list of ${repoName}: $($_.Exception.Message); without it a rerun could duplicate issues, so nothing was filed"
        return 1
    }
    if ($listed.Count -ge $issueListLimit) {
        Write-Host "the issue list has $($listed.Count) issues, the most this script reads at once, so it can't tell what is already filed; nothing was filed"
        return 1
    }
    $numbers = @{}
    foreach ($finding in $parsed.Findings) {
        $marker = "<!-- review-finding: $postedReviewName#$($finding.Id) -->"
        $hit = @($listed | Where-Object { $_.body -and $_.body.Contains($marker) })
        if ($hit) { $numbers[$finding.Id] = [int]$hit[0].number }
    }
    $existing = @($numbers.Keys)
    $listedByNumber = @{}
    foreach ($id in $existing) {
        $issue = $listed | Where-Object { [int]$_.number -eq $numbers[$id] } | Select-Object -First 1
        $listedByNumber["$($numbers[$id])"] = [string]$issue.body
    }
    $toCreate = @($prepared | Where-Object { $existing -notcontains $_.Id })

    $view = $parsed.PSObject.Copy()
    # Backlinks (Related: S-1 depends on this one) come from the whole parsed review, not only the selected
    # findings, so a later -Only batch still links an earlier-filed dependent. An excluded finding is never
    # refused, so only its protected id can reach a body; one whose id looks like a secret is left out.
    $preparedById = @{}
    foreach ($p in $prepared) { $preparedById[$p.Id] = $p }
    $view.Findings = @(foreach ($finding in $parsed.Findings) {
            if ($preparedById.ContainsKey($finding.Id)) { $preparedById[$finding.Id]; continue }
            if (Find-SecretKind $finding.Id) { continue }
            $other = Protect-Finding $finding -KeepPaths:$KeepPaths 3>$null
            $other.Id = Protect-Text $finding.Id -Id $finding.Id -KeepPaths:$KeepPaths 3>$null
            $other
        })
    if ($scopeFallback) { $view.Scope = $scopeFallback }
    $render = {
        param($finding)
        New-IssueBody -Review $view -Finding $finding -Repo $repoName -ReviewName $postedReviewName -HeadSha $headSha `
            -IdMap $numbers -StandardCriteria:$StandardCriteria
    }
    if (-not $Create) {
        foreach ($finding in $prepared) {
            if ($numbers.ContainsKey($finding.Id)) {
                Write-Host "$($finding.Id) skipped (exists #$($numbers[$finding.Id]))"
                continue
            }
            Write-Host "would create [$($finding.Label)] $($finding.Id): $($finding.Title)"
            Write-Host (& $render $finding)
            Write-Host '---'
        }
        Write-Host "not filed: $(Get-Plural $parsed.NotFiled.Sections 'section'), $(Get-Plural $parsed.NotFiled.Nits 'nit')"
        Write-Host ('would write: ' + (Format-IssuesLine $parsed.Findings $numbers $toCreate.Id))
        return 0
    }

    $created = [ordered]@{}
    $failure = $null
    foreach ($finding in (Get-FilingOrder $toCreate)) {
        $body = & $render $finding
        try {
            $url = Invoke-GhWithBody { param($file) gh issue create --repo $repoName --title "$($finding.Id): $($finding.Title)" --label $finding.Label --body-file $file } $body
        }
        catch { $failure = "$($finding.Id): $($_.Exception.Message)"; break }
        $number = [int]([regex]::Match("$url", '/issues/(\d+)\s*$').Groups[1].Value)
        $numbers[$finding.Id] = $number
        $created[$finding.Id] = @{ Number = $number; Url = "$url".Trim(); Body = $body; Finding = $finding }
    }
    # Every number is known now: bodies that held an id for a later issue are rewritten with the #n.
    foreach ($id in $created.Keys) {
        $entry = $created[$id]
        $final = & $render $entry.Finding
        if ($final -ne $entry.Body) {
            try { Invoke-GhWithBody { param($file) gh issue edit $entry.Number --repo $repoName --body-file $file } $final | Out-Null }
            catch { $failure = "#$($entry.Number) links: $($_.Exception.Message)" }
        }
    }

    # Issues that existed before this run get only their link fields patched, from the body as listed.
    foreach ($item in ($listedByNumber.GetEnumerator() | Sort-Object Name)) {
        $patched = Update-LinkFields $item.Value $numbers
        if ($patched -eq $item.Value) { continue }
        try { Invoke-GhWithBody { param($file) gh issue edit ([int]$item.Name) --repo $repoName --body-file $file } $patched | Out-Null }
        catch { $failure = "#$($item.Name) links: $($_.Exception.Message)" }
    }

    Set-IssuesLine $resolvedPath (Format-IssuesLine $parsed.Findings $numbers)

    foreach ($finding in $prepared) {
        $status = if ($created.Contains($finding.Id)) { 'created' } elseif ($numbers.ContainsKey($finding.Id)) { 'skipped (exists)' } else { 'not filed' }
        $number = if ($numbers.ContainsKey($finding.Id)) { "#$($numbers[$finding.Id])" } else { '-' }
        $url = if ($numbers.ContainsKey($finding.Id)) { "https://github.com/$repoName/issues/$($numbers[$finding.Id])" } else { '-' }
        Write-Host ("{0,-8} {1,-11} {2,-5} {3} {4}" -f $finding.Id, $finding.Label, $number, $url, $status)
    }
    Write-Host "not filed: $(Get-Plural $parsed.NotFiled.Sections 'section'), $(Get-Plural $parsed.NotFiled.Nits 'nit')"
    if ($failure) {
        Write-Host "stopped early: $failure; rerun to file the rest (existing issues are skipped)"
        if ($created.Count) { return 2 } else { return 1 }
    }
    0
}
