236. **A lock file read from a commit is capped at 16 MiB (2026-10-21, milestone 3 slice 8 review M3S8-3, the user
chose the limit; like [198]):** `Locks::Commit` asks `git cat-file -s` for a lock file's size before reading it. One
over 16 MiB is not read and counts as differing from the warm template's, with a reason naming the file and its size:
a strict profile is refused, a non-strict one warns and installs. A PR's head is written by someone else, so its lock
file must not be read into memory whole without a bound.
