213. **A separate minimum `sbx` version for templates (2026-10-15, PR 253 review Q-3, the user agreed with the
recommendation):** templates need `sbx` 0.46.0, the version S11 tested, built into sbxm as a constant (not a config
key: below the tested version is unsupported). `doctor` checks it only when a profile has `[template]`; `template
build` refuses below it before writing anything; a create that would use a template treats an older `sbx` like a
missing template: refused for a strict profile [200], today's installs plus a warning otherwise [203]. The global
`min_sbx_version` (default 0.43.0, `src/config.rs`) stays for everything else, and the README says which minimum
applies to what. Why: raising the global minimum would make `doctor` fail for profiles without `[template]`,
against [202].
