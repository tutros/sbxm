256. **Provider logins stay global; every other secret is scoped to its sandbox (2026-10-23, the user's answer;
builds on [30], [241]; refines [248]):** this refines [248]'s conclusion that "global cloud secrets reach every cloud
sandbox, so each one is scoped to its sandbox": provider OAuth logins are the exception and stay global, and every
other secret is still scoped. The provider OAuth secrets (`sbx secret set <provider> --oauth --cloud`) stay at
global scope. Their value never enters a sandbox (S14 Q2, Q7), and egress lets a sandbox reach only its harness's own
hosts. Preflight checks that the harness's provider secret exists in the cloud. Every other secret a profile names is
scoped to the sandbox's name before `create` and removed after the sandbox is gone, the pattern of Jev's scoped
secret [241]. Grounds: S14 Q7 showed global cloud secrets reach every cloud sandbox, which a provider login's
value never enters; a scoped secret reaches only the sandbox it names.
