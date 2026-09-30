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
        HeadSha = $null; Scope = $null; IssuesLine = $null
    }
    $section = $null     # @{ Label; Findings = count }
    $finding = $null
    $sections = [System.Collections.Generic.List[object]]::new()
    for ($i = 0; $i -lt $lines.Count; $i++) {
        $line = $lines[$i]
        if ($line -match '^Scope:\s*(.*)$' -and -not $review.Scope) {
            $review.Scope = $Matches[1]
            $sha = [regex]::Match($Matches[1], '\b[0-9a-f]{7,40}\.\.([0-9a-f]{7,40})\b')
            if ($sha.Success) { $review.HeadSha = $sha.Groups[1].Value }
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
