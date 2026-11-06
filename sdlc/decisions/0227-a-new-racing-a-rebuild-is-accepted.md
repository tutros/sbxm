227. **A `new` racing a rebuild is accepted (2026-10-17, milestone 3 slice 4, the user accepted the risk):** a
sandbox creation that reads the template record just before a rebuild rewrites it, and records its tag just after the
rebuild checked for users, can lose its template to [205](a)'s removal. The window is milliseconds between two local
commands; no lock covers `new`.
