#!/bin/sh
# Counts the `git` process starts of one test file (issue #444). Run by hand, never in CI:
#   scripts/count-git-starts.sh task_merged        (or: just count-git-starts task_merged)
# A counting `git` wrapper goes first on PATH and logs one line per start, then runs the real git.
# Callers: GIT_AUTHOR_NAME "t" is a test fixture (tests/common/), anything else is sbxm's own code.
set -eu
test_name=${1:?usage: count-git-starts.sh <test file name, e.g. task_merged>}
real_git=$(command -v git)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir "$work/bin"
cat > "$work/bin/git" <<WRAP
#!/bin/sh
if [ "\${GIT_AUTHOR_NAME:-}" = "t" ]; then who=fixture; else who=sbxm; fi
sub=""; skip=0
for a in "\$@"; do
  if [ \$skip = 1 ]; then skip=0; continue; fi
  case "\$a" in --git-dir|--work-tree|-c|-C) skip=1;; -*) ;; *) sub="\$a"; break;; esac
done
echo "\$who \$sub" >> "\$COUNTLOG_GITSTARTS"
exec "$real_git" "\$@"
WRAP
chmod +x "$work/bin/git"
COUNTLOG_GITSTARTS="$work/starts.log"
export COUNTLOG_GITSTARTS
: > "$COUNTLOG_GITSTARTS"
# Build first with the real git, so only the test run is counted.
cargo test --test "$test_name" --no-run
PATH="$work/bin:$PATH" cargo test --test "$test_name" >/dev/null 2>&1 || echo "note: the test run failed" >&2
echo "git starts by caller:"
awk '{n[$1]++} END {for (k in n) printf "%6d %s\n", n[k], k}' "$COUNTLOG_GITSTARTS" | sort -k2
echo "git starts by caller and subcommand:"
sort "$COUNTLOG_GITSTARTS" | uniq -c | sort -k2,2 -k1,1nr
