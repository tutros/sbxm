//! `sbxm task file-findings (--issue N | --pr N | --file F) [--create]` (spec §4, §11, decisions
//! 134, 149): files the findings of a review as GitHub issues. A dry run unless `--create`.
//! Everything that can be wrong is checked before the first write: ids, the `Issues:` line,
//! secrets, labels, the issue list. Ported from `scripts/file-review-issues.ps1`.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::{Context, Result, anyhow, bail};
use regex::Regex;

use crate::commands::task_start::{check_repo, github_repo};
use crate::git;
use crate::github::{GitHubBackend, IssueRequest};
use crate::task::findings::{
    self, Finding, Render, acceptance_criteria, filing_order, find_secrets, issue_body,
    issues_line, marker, protect_finding, protect_text, secret_kind, update_link_fields,
};

/// How many issues one listing reads. A repo with this many or more can't be checked for
/// markers, so the command refuses instead of risking a duplicate.
const ISSUE_LIST_LIMIT: u32 = 1000;

pub enum Source {
    /// Any review file, e.g. `sdlc/reviews/<date>-<scope>.md`.
    File(PathBuf),
}

pub struct Options {
    pub source: Source,
    /// Where `origin` is read (and a short commit sha resolved) when the repo isn't named.
    pub repo_root: PathBuf,
    /// `owner/name`. Default: the `origin` of the checkout at `repo_root`.
    pub repo: Option<String>,
    /// Publish the issues. Without it nothing is created or edited.
    pub create: bool,
    /// File findings with no acceptance criteria with only the standard ones.
    pub standard_criteria: bool,
    /// Keep personal paths instead of replacing them with `~`.
    pub keep_paths: bool,
    /// File only these finding ids.
    pub only: Vec<String>,
}

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// A full sha stays as it is, a short one is resolved by git; nothing in, nothing out.
fn resolve_head_sha(sha: Option<&str>, cwd: &Path, warnings: &mut Vec<String>) -> Option<String> {
    let sha = sha?;
    if sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return Some(sha.to_owned());
    }
    match git::user_run(
        cwd,
        &["rev-parse", "--verify", &format!("{sha}^{{commit}}")],
    ) {
        Ok(full) if !full.trim().is_empty() => Some(full.trim().to_owned()),
        _ => {
            warnings.push(format!(
                "git can't resolve the reviewed commit {sha} here; Where links stay plain text"
            ));
            None
        }
    }
}

/// Files (or, as a dry run, only shows) the issues for a review's findings. What it prints goes
/// to `out`; warnings, each once, to `warn`.
pub fn run(
    opts: &Options,
    github: &dyn GitHubBackend,
    out: &mut dyn Write,
    warn: &mut dyn Write,
) -> Result<()> {
    let mut warnings = Vec::new();
    let result = file(opts, github, out, &mut warnings);
    let mut seen: Vec<&String> = Vec::new();
    for warning in &warnings {
        if !seen.contains(&warning) {
            writeln!(warn, "warning: {warning}")?;
            seen.push(warning);
        }
    }
    result
}

fn file(
    opts: &Options,
    github: &dyn GitHubBackend,
    out: &mut dyn Write,
    warnings: &mut Vec<String>,
) -> Result<()> {
    let Source::File(path) = &opts.source;
    if !path.is_file() {
        bail!(
            "review file {} not found; give the path of a file in sdlc/reviews/",
            path.display()
        );
    }
    let text = fs::read_to_string(path).with_context(|| {
        format!(
            "cannot read {}; check the file and its permissions",
            path.display()
        )
    })?;
    let name = path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let parsed = findings::parse(&text);

    // An id goes into titles, bodies, markers and the Issues line, so it can't be a way to carry
    // a path or a secret into an issue. Checked before any GitHub call, for every finding.
    let bad: Vec<&Finding> = parsed
        .findings
        .iter()
        .filter(|f| !valid_id(&f.id))
        .collect();
    if !bad.is_empty() {
        for f in bad {
            writeln!(
                out,
                "refused: finding id {} (line {} of {name}) has characters outside A-Za-z0-9._-; rename it in the review file",
                f.id, f.start_line
            )?;
        }
        bail!("nothing was filed or changed: fix the finding ids in {name} first");
    }

    // A duplicate id would let two blocks both reach `issue_create`, the second overwriting the
    // first's number in the write-back.
    let mut dupes = false;
    let mut reported: Vec<&str> = Vec::new();
    for f in &parsed.findings {
        let same: Vec<&Finding> = parsed.findings.iter().filter(|g| g.id == f.id).collect();
        if same.len() > 1 && !reported.contains(&f.id.as_str()) {
            reported.push(&f.id);
            dupes = true;
            let lines: Vec<String> = same.iter().map(|g| g.start_line.to_string()).collect();
            writeln!(
                out,
                "refused: {} is used by more than one finding in {name}, at lines {}",
                f.id,
                lines.join(" and ")
            )?;
        }
    }
    if dupes {
        bail!("nothing was filed or changed: make finding ids unique in {name} first");
    }

    // The write-back needs exactly one `Issues:` line to replace.
    if parsed.issues_line_count != 1 {
        let what = match parsed.issues_line_count {
            0 => "has no".to_owned(),
            n => format!("has {n}"),
        };
        bail!(
            "{name} {what} 'Issues:' line(s); keep exactly one so issue numbers can be written back"
        );
    }

    // A file a rerun can't update is caught before any issue is published.
    if opts.create && fs::OpenOptions::new().write(true).open(path).is_err() {
        bail!("can't update {name} to record issue numbers; check its file permissions and rerun");
    }

    let ids: Vec<&str> = parsed.findings.iter().map(|f| f.id.as_str()).collect();
    let unknown: Vec<&str> = opts
        .only
        .iter()
        .map(String::as_str)
        .filter(|o| !ids.contains(o))
        .collect();
    if !unknown.is_empty() {
        bail!(
            "--only names {}, which isn't in {name}; its findings are {}",
            unknown.join(", "),
            ids.join(", ")
        );
    }
    let selected: Vec<&Finding> = parsed
        .findings
        .iter()
        .filter(|f| opts.only.is_empty() || opts.only.contains(&f.id))
        .collect();
    if selected.is_empty() {
        bail!(
            "no findings found in {name}; check its headings against the review file format in the code review skill"
        );
    }

    // The repo, who is logged in, and the labels the selected findings need.
    let repo = match &opts.repo {
        Some(repo) => {
            check_repo(repo)?;
            repo.clone()
        }
        None => {
            let url =
                git::user_run(&opts.repo_root, &["remote", "get-url", "origin"]).map_err(|e| {
                    anyhow!(
                        "cannot read the origin of {}: {e:#}; pass --repo owner/name",
                        opts.repo_root.display()
                    )
                })?;
            github_repo(url.trim())?
        }
    };
    let stays = "nothing was filed or changed; the review stays marked 'Issues: pending (no GitHub access from this host)'";
    github
        .whoami()
        .map_err(|_| anyhow!("gh isn't logged in; run `gh auth login` and rerun ({stays})"))?;
    let existing_labels = github.labels(&repo).map_err(|e| {
        anyhow!(
            "gh can't read the repo {repo}: {e:#}; check the name and run `gh auth status` ({stays})"
        )
    })?;
    let mut needed: Vec<&str> = selected.iter().map(|f| f.label.as_str()).collect();
    needed.sort_unstable();
    needed.dedup();
    for label in needed {
        if !existing_labels.iter().any(|l| l == label) {
            bail!(
                "label '{label}' doesn't exist in {repo}; create it (this command creates no labels; {stays})"
            );
        }
    }

    let head_sha = resolve_head_sha(parsed.head_sha.as_deref(), &opts.repo_root, warnings);
    let mut problems: Vec<String> = Vec::new();

    // The file name and, when there is no commit sha, the Scope line's fallback text go into
    // every issue body, so they get the same refusal and protection as a finding's own fields.
    let name_secret = secret_kind(&name);
    if let Some(kind) = name_secret {
        problems.push(format!(
            "the review file name looks like {kind}; rename the file"
        ));
    }
    let posted_name = match name_secret {
        Some(_) => name.clone(),
        None => protect_text(&name, "the review file name", opts.keep_paths, warnings),
    };
    let mut scope_fallback = None;
    if parsed.range.is_none() {
        let scope = parsed.scope.as_deref().unwrap_or("");
        match secret_kind(scope) {
            Some(kind) => problems.push(format!(
                "Scope line {} looks like {kind}; remove it from the review file",
                parsed.scope_line.unwrap_or(0)
            )),
            None => scope_fallback = Some(protect_text(scope, "Scope", opts.keep_paths, warnings)),
        }
    }

    let mut prepared: Vec<Finding> = Vec::new();
    for finding in &selected {
        let before = problems.len();
        if !finding.missing.is_empty() {
            problems.push(format!(
                "{} is missing: {}",
                finding.id,
                finding.missing.join(", ")
            ));
        }
        if let Err(message) = acceptance_criteria(finding, opts.standard_criteria) {
            problems.push(message);
        }
        for hit in find_secrets(&text, finding) {
            problems.push(format!(
                "{} line {} looks like {}; remove it from the review file",
                hit.id, hit.line, hit.kind
            ));
        }
        if problems.len() == before {
            prepared.push(protect_finding(finding, opts.keep_paths, warnings));
        }
    }
    if !problems.is_empty() {
        for problem in &problems {
            writeln!(out, "refused: {problem}")?;
        }
        let verb = if opts.create {
            "nothing was filed"
        } else {
            "nothing would be filed"
        };
        bail!(
            "{verb}: {} to fix in {name} first",
            plural(problems.len(), "problem")
        );
    }

    // What exists already: a finding whose marker is in an issue body keeps that number.
    let listed = github.issues_all(&repo, ISSUE_LIST_LIMIT).map_err(|e| {
        anyhow!("can't read the issue list of {repo}: {e:#}; without it a rerun could duplicate issues, so nothing was filed")
    })?;
    if listed.len() >= ISSUE_LIST_LIMIT as usize {
        bail!(
            "the issue list has {} issues, the most this command reads at once, so it can't tell what is already filed; nothing was filed",
            listed.len()
        );
    }
    let mut numbers: BTreeMap<String, u32> = BTreeMap::new();
    for finding in &parsed.findings {
        let wanted = marker(&posted_name, &finding.id);
        if let Some(issue) = listed.iter().find(|i| i.body.contains(&wanted)) {
            numbers.insert(finding.id.clone(), issue.number);
        }
    }
    let to_create: Vec<&Finding> = prepared
        .iter()
        .filter(|f| !numbers.contains_key(&f.id))
        .collect();

    // Backlinks (Related: S-1 depends on this one) come from the whole parsed review, not only
    // the selected findings, so a later `--only` batch still links an earlier-filed dependent. An
    // excluded finding is never refused, so only its protected text can reach a body; one whose
    // id looks like a secret is left out.
    let mut view = parsed.clone();
    view.findings = parsed
        .findings
        .iter()
        .filter_map(|finding| {
            if let Some(done) = prepared.iter().find(|p| p.id == finding.id) {
                return Some(done.clone());
            }
            if secret_kind(&finding.id).is_some() {
                return None;
            }
            let mut other = protect_finding(finding, opts.keep_paths, &mut Vec::new());
            other.id = protect_text(&finding.id, &finding.id, opts.keep_paths, &mut Vec::new());
            Some(other)
        })
        .collect();
    if let Some(scope) = scope_fallback {
        view.scope = Some(scope);
    }
    let review_ref = format!("sdlc/reviews/{posted_name}");
    let mut render = |finding: &Finding, ids: &BTreeMap<String, u32>| -> Result<String> {
        let context = Render {
            repo: &repo,
            review_name: &posted_name,
            review_ref: &review_ref,
            head_sha: head_sha.as_deref(),
            ids,
            standard_criteria: opts.standard_criteria,
        };
        issue_body(&view, finding, &context, warnings).map_err(|e| anyhow!(e))
    };

    if !opts.create {
        for finding in &prepared {
            if let Some(number) = numbers.get(&finding.id) {
                writeln!(out, "{} skipped (exists #{number})", finding.id)?;
                continue;
            }
            writeln!(
                out,
                "would create [{}] {}: {}",
                finding.label, finding.id, finding.title
            )?;
            writeln!(out, "{}", render(finding, &numbers)?)?;
            writeln!(out, "---")?;
        }
        writeln!(
            out,
            "not filed: {}, {}",
            plural(parsed.not_filed_sections, "section"),
            plural(parsed.not_filed_nits, "nit")
        )?;
        let would: Vec<String> = to_create.iter().map(|f| f.id.clone()).collect();
        writeln!(
            out,
            "would write: {}",
            issues_line(&parsed.findings, &numbers, &would, "#?")
        )?;
        return Ok(());
    }

    // The bodies of issues that exist already, as listed, for patching their links at the end.
    let existing: BTreeMap<u32, String> = listed
        .iter()
        .filter(|i| numbers.values().any(|n| *n == i.number))
        .map(|i| (i.number, i.body.clone()))
        .collect();

    let mut created: Vec<(Finding, u32, String)> = Vec::new();
    let mut failure: Option<String> = None;
    let todo: Vec<Finding> = to_create.iter().map(|f| (*f).clone()).collect();
    for finding in filing_order(&todo) {
        let body = render(finding, &numbers)?;
        let request = IssueRequest {
            title: format!("{}: {}", finding.id, finding.title),
            body: body.clone(),
            labels: vec![finding.label.clone()],
        };
        match github.issue_create(&repo, &request) {
            Ok(number) => {
                numbers.insert(finding.id.clone(), number);
                created.push((finding.clone(), number, body));
            }
            Err(e) => {
                failure = Some(format!("{}: {e:#}", finding.id));
                break;
            }
        }
    }
    // Every number is known now: bodies that held an id for a later issue are rewritten with #n.
    for (finding, number, body) in &created {
        let last = render(finding, &numbers)?;
        if last != *body
            && let Err(e) = github.issue_edit(&repo, *number, &last)
        {
            failure = Some(format!("#{number} links: {e:#}"));
        }
    }
    // Issues that existed before this run get only their link fields patched, from the body as
    // listed.
    for (number, body) in &existing {
        let patched = update_link_fields(body, &numbers);
        if patched != *body
            && let Err(e) = github.issue_edit(&repo, *number, &patched)
        {
            failure = Some(format!("#{number} links: {e:#}"));
        }
    }

    set_issues_line(
        path,
        &issues_line(&parsed.findings, &numbers, &[], "#?"),
        warnings,
    )?;

    for finding in &prepared {
        let status = if created.iter().any(|(f, ..)| f.id == finding.id) {
            "created"
        } else if numbers.contains_key(&finding.id) {
            "skipped (exists)"
        } else {
            "not filed"
        };
        let (number, url) = match numbers.get(&finding.id) {
            Some(n) => (
                format!("#{n}"),
                format!("https://github.com/{repo}/issues/{n}"),
            ),
            None => ("-".to_owned(), "-".to_owned()),
        };
        writeln!(
            out,
            "{:<8} {:<11} {:<5} {url} {status}",
            finding.id, finding.label, number
        )?;
    }
    writeln!(
        out,
        "not filed: {}, {}",
        plural(parsed.not_filed_sections, "section"),
        plural(parsed.not_filed_nits, "nit")
    )?;
    if let Some(failure) = failure {
        bail!("stopped early: {failure}; rerun to file the rest (existing issues are skipped)");
    }
    Ok(())
}

/// Replaces the first `Issues:` line and nothing else: the file's line endings and byte order
/// mark stay.
fn set_issues_line(path: &Path, line: &str, warnings: &mut Vec<String>) -> Result<()> {
    static ISSUES: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?m)^Issues:[^\r\n]*").expect("a valid pattern"));
    let bytes = fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    let bom = bytes.starts_with(&[0xEF, 0xBB, 0xBF]);
    let text = std::str::from_utf8(&bytes[if bom { 3 } else { 0 }..])
        .with_context(|| format!("{} isn't UTF-8", path.display()))?;
    if !ISSUES.is_match(text) {
        warnings.push(format!(
            "{} has no 'Issues:' line to update; add one so the issue numbers are recorded",
            path.display()
        ));
        return Ok(());
    }
    let new = ISSUES.replace(text, regex::NoExpand(line));
    let mut out = Vec::with_capacity(bytes.len());
    if bom {
        out.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
    }
    out.extend_from_slice(new.as_bytes());
    fs::write(path, out).with_context(|| {
        format!(
            "cannot update {} to record the issue numbers; check its file permissions",
            path.display()
        )
    })
}
