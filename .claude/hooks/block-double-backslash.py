r"""PreToolUse hook for the Bash tool: block commands containing a double backslash.

On this machine the Bash tool turns every `\\` in a command into `\` before
bash runs it, even inside single quotes and quoted heredocs, so such a command
never runs as written (e.g. a Rust "\n" escape written through a heredoc ends
up as a real line break). A lone `\` is unaffected, and the PowerShell tool
keeps backslashes intact.

Exit code 2 blocks the call and shows stderr to Claude.
"""

import json
import sys

command = json.load(sys.stdin).get("tool_input", {}).get("command", "")
if "\\\\" in command:
    sys.stderr.write(
        "Blocked: this Bash command contains a double backslash, which the Bash "
        "tool collapses to a single one, so it would not run as written. Write "
        "file contents with the Write/Edit tools, or run the command with the "
        "PowerShell tool, which keeps backslashes.\n"
    )
    sys.exit(2)
