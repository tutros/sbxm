# The functions behind file-review-issues.ps1 (decision 134): parse a review file from reviews/, render one issue
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
        HeadSha = $null; Range = $null; Scope = $null; IssuesLine = $null
    }
    $section = $null     # @{ Label; Findings = count }
    $finding = $null
    $sections = [System.Collections.Generic.List[object]]::new()
    for ($i = 0; $i -lt $lines.Count; $i++) {
        $line = $lines[$i]
        if ($line -match '^Scope:\s*(.*)$' -and -not $review.Scope) {
            $review.Scope = $Matches[1]
            $sha = [regex]::Match($Matches[1], '\b[0-9a-f]{7,40}\.\.([0-9a-f]{7,40})\b')
            if ($sha.Success) { $review.HeadSha = $sha.Groups[1].Value; $review.Range = $sha.Value }
        }
        elseif ($line -match '^Issues:' -and -not $review.IssuesLine) { $review.IssuesLine = $line }
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
    $finding.Remove('Body')
    $finding
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

# `src/a.rs:10-20` becomes a permalink at the reviewed commit. Plain words are left alone.
function ConvertTo-WhereLinks {
    param([string]$Text, [string]$Repo, [string]$Sha)
    $known = 'rs|toml|md|ps1|psm1|json|ya?ml|lock|txt|sh|py|js|ts'
    $pattern = '(?<![\w/.:-])(`?)((?:[\w.-]+/)*[\w.-]+\.\w+)(?::(\d+)(?:-(\d+))?)?(`?)'
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
    $parts.Add("**Review:** ``reviews/$ReviewName``, finding $($Finding.Id), reviewed commits ``$range``")
    $parts.Add("<!-- review-finding: $ReviewName#$($Finding.Id) -->")
    ($parts -join "`n") + "`n"
}
