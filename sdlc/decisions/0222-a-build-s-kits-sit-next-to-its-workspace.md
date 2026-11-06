222. **A build's kits sit next to its workspace (2026-10-17, milestone 3 slice 4, announced to the user; adds to
[210]):** `template build` writes the build sandbox's install kits to
`<base>/.sbxm/templates/build/<profile>-<harness>.kits/` and removes them with the workspace after every build. Why:
the kits are build leftovers like the workspace, so [212]'s cleanup finds both by the same fixed name.
