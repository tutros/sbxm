226. **An interrupted build's saved tag is not cleaned up automatically (2026-10-17, milestone 3 slice 4, the user
chose to defer):** a build killed between `sbx template save` and writing its record leaves an image no record names;
[212]'s leftover cleanup removes only the build sandbox and folders. Trigger to fix: such an image is found on this
machine (`sbx template ls` shows an `sbxm-` tag no record lists), or `template ls`/`doctor` (slices 7, 8) can list
orphans cheaply.
