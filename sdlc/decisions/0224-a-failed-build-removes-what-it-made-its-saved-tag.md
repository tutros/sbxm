224. **A failed build removes what it made, its saved tag included (2026-10-17, milestone 3 slice 4, the user
agreed):** once the kits are written, a failure removes the build sandbox (also after a failed `create`, which can
leave one), a tag saved without its record, and the build folders; a cleanup step that fails is added to the error
with the command that finishes it (`sbx rm`, `sbx template rm`, delete by hand). Why: an image no record names is
1.3-2.6 GB that nothing would ever find or remove ([205]).
