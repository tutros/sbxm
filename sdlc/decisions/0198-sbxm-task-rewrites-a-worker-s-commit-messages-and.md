198. **`sbxm task` rewrites a worker's commit messages, and refuses a commit over 1 MiB (2026-10-15, decided with
the user, issue 244, PR 250):** before the gates, the review or any sink see them, the agent's commits are rebuilt
from a separate agent ref (`refs/sbxm/agent/<branch>`) onto the delivered branch with every closing reference and
mention neutralised by `review::defang`, deterministically and in one atomic ref move, with hooks off; only commits
the agent made are rebuilt, and signatures are dropped (the issue's "Decided" section has the rest). Why: a closing
keyword in a commit message closes that issue once the commit lands on `main`, and PRs here are merged with merge
commits. A commit object over 1 MiB (header plus message) is refused before it is read, not rewritten: the rebuild
holds several copies of a message (about 26 bytes of memory per message byte, measured in the PR 250 review), and a
highly compressible message is tiny in the bundle. Streaming the rewrite instead is deferred to issue 251, with the
trigger of a real worker commit hitting the cap. This narrows the issue's "rewrite, not refuse" for that one case.
The cap bounds sbxm's own copies, not git's: the fetch and the range walk still unpack an oversized object before the
check, at about one byte of memory per unpacked byte, as the fetch already did for every object before issue 244.
Limiting the unpacked size of a bundle's objects is deferred to issue 252 (PR 250 review round 3, M-1).
