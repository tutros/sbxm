<#
.SYNOPSIS
Files the findings of a code review file from sdlc/reviews/ as GitHub issues (decision 134).

.DESCRIPTION
Reviews run in sandboxes with no GitHub access, so their findings wait in sdlc/reviews/<date>-<scope>.md marked
"Issues: pending". Run this from a host where gh works. It parses the file (the format is in the code review skill,
section 8; Codex's variants are accepted too), checks access, scrubs each finding, and files one issue per finding
with the skill's template, dependencies linked both ways and permalinks at the reviewed commit.

Without -Create it is a dry run: it prints every issue it would create and the Issues line it would write, and
changes nothing, because the repo is public. With -Create it files them, skips findings that already have an
issue (a hidden marker in each body), so a rerun never duplicates, then rewrites the review file's Issues line.

Secrets in a finding refuse the whole run. Personal paths (C:\Users\<name>, /home/<name>) are replaced with ~.
No labels are created: must-fix, should-fix and question must exist.

Exit codes: 0 all done; 1 nothing filed (parse error, access failure, refused content); 2 some filed, some not.

.PARAMETER Review
Path of the review file, e.g. sdlc/reviews/2026-09-30-milestone-2a.md.

.PARAMETER Repo
owner/name. Defaults to the repo that gh reports for this checkout's GitHub origin.

.PARAMETER Create
Publish the issues. Without it nothing is created or edited.

.PARAMETER StandardCriteria
File findings that have no acceptance criteria with only the standard ones, plus a first line saying the specific
check is missing. Without it such a finding is refused.

.PARAMETER KeepPaths
Keep personal paths as they are instead of replacing them with ~.

.PARAMETER Only
File only these finding ids, e.g. -Only M2A-1,M2A-2.

.EXAMPLE
./scripts/file-review-issues.ps1 sdlc/reviews/2026-09-30-milestone-2a.md -StandardCriteria

Dry run: shows the four issues and the Issues line, files nothing.

.EXAMPLE
./scripts/file-review-issues.ps1 sdlc/reviews/2026-09-30-milestone-2a.md -StandardCriteria -Create -Only M2A-1

Files one finding; run it again to see "skipped (exists)", then drop -Only for the rest.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory, Position = 0)][string]$Review,
    [string]$Repo,
    [switch]$Create,
    [switch]$StandardCriteria,
    [switch]$KeepPaths,
    [string[]]$Only
)
$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'file-review-issues.psm1') -Force

$code = @(Invoke-ReviewFiling @PSBoundParameters)[-1]
exit $code
