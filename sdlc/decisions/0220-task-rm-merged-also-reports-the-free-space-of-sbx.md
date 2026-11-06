220. **`task rm --merged` also reports the free space of `sbx`'s sandbox state drive (2026-10-16,
decided with the user, one point at a time, recorded on issue 258; workflow gap G51):** [195]'s
"freed about" line names only the base dir's drive, but a task's sandboxes take their space on the
drive `disk::state_dir` resolves (`sandbox_state_dir` or the platform default, [194]), so that line
alone said nothing about what a removal actually freed (trigger: removing the merged tasks for
issues 179 and 98 printed "freed about 0.0 GiB on drive H:" while C: gained about 20 GiB). `task rm
--merged` now reads and reports that drive too, before and after, the same way as the base dir: left
out when the folder isn't known on this platform or its free space can't be read, exactly as an
unreadable base dir already is. When the base dir and the state dir are on the same drive (the
same path, or on Windows the same drive letter), only one line is printed (the base dir's), so the
same free space isn't counted twice.
