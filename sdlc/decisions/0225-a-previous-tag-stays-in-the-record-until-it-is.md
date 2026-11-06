225. **A previous tag stays in the record until it is gone (2026-10-17, milestone 3 slice 4, the user agreed; adds to
[205](a)):** the new record lists the old tag and every kept `previous` tag when it is written; only then are they
checked and removed. A tag in use, one that `sbx template rm` fails to remove, or one whose use can't be checked (an
unreadable `task.json`) stays in `previous` and the next build tries again; a tag whose image is already gone is
dropped silently. `template build` prints the new tag, then one line per previous tag removed or kept, with the
reason, on stdout. Why: a sandbox created during the build still finds its tag recorded, and nothing is lost to a
failed removal.
