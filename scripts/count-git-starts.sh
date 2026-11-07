#!/bin/sh
# Counts the `git` process starts of one test file (issue #444). Run by hand, never in CI:
#   scripts/count-git-starts.sh task_merged        (or: just count-git-starts task_merged)
#   scripts/count-git-starts.sh task_merged 395    (exits 1 when sbxm's own starts are over 395)
# A counting `git` wrapper goes first on PATH and logs one line per start, then runs the real git.
# Callers: GIT_AUTHOR_NAME "t" is a test fixture (tests/common/), anything else is sbxm's own code.
set -eu
test_name=${1:?usage: count-git-starts.sh <test file name, e.g. task_merged> [max sbxm starts]}
max=${2:-}
case "$max" in
  ''|*[!0-9]*) if [ -n "$max" ]; then echo "max must be a number, not '$max'" >&2; exit 2; fi;;
esac
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
run_status=0
PATH="$work/bin:$PATH" cargo test --test "$test_name" >/dev/null 2>&1 || run_status=$?
if [ "$run_status" -ne 0 ]; then echo "the test run failed (exit $run_status); the counts below are partial" >&2; fi
echo "git starts by caller:"
awk '{n[$1]++} END {for (k in n) printf "%6d %s\n", n[k], k}' "$COUNTLOG_GITSTARTS" | sort -k2
echo "git starts by caller and subcommand:"
sort "$COUNTLOG_GITSTARTS" | uniq -c | sort -k2,2 -k1,1nr
if [ "$run_status" -ne 0 ]; then
  exit 1
fi
if [ -n "$max" ]; then
  sbxm_starts=$(awk '$1 == "sbxm" {n++} END {print n + 0}' "$COUNTLOG_GITSTARTS")
  if [ "$sbxm_starts" -gt "$max" ]; then
    echo "sbxm made $sbxm_starts git starts, over the limit of $max" >&2
    exit 1
  fi
  echo "sbxm made $sbxm_starts git starts, within the limit of $max"
fi
