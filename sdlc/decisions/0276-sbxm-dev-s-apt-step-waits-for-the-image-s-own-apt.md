276. **`sbxm-dev`'s apt step waits for the image's own `apt-get` (2026-10-31, the user's choice; issue #337, gap
G67):** the cloud failure at create wasn't the allow list. A probe kit reached `archive.ubuntu.com` (HTTP 200) during
its install, so the list is in force then. The cloud image runs its own `apt-get` at startup; the install step's
`apt-get update` failed at once on the package lists' lock (`DPkg::Lock::Timeout` covers only dpkg's lock), `|| true`
hid it, and `install` found no packages (exit 100). The fix is in the profile, not in sbxm: before `update`, the step
polls every 2 s while `pgrep -x apt-get` finds one, for at most 5 minutes, then goes on either way. Proven on the
cloud on 2026-10-31: it waited 6 s, then installed. The small race (another `apt-get` starting between the check and
`update`) is left until it is seen.
