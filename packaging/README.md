# Hunkra local Fedora build

Start Hunkra from the application menu or run `/usr/bin/hunkra /absolute/repository`.
Run `/usr/bin/hunkra --version` without a display to inspect the product version.
English, Russian and programmer Russian resources are embedded; no source checkout,
network connection or Zed installation is needed to display the interface.

Changes shows separate HEAD→index and index→worktree diffs with file/hunk/line staging.
Commit uses the reviewed index. Branches controls switch/merge/rebase and recovered
Continue/Abort/Skip. Conflicts have independent Result editing: Save, Mark resolved
and Continue are separate. History uses explicit parent comparisons; Remotes provides
Fetch, explicit branch Push/upstream and fast-forward-only Pull. No automatic stash,
reset, discard or force push is performed. Unsupported index/encoding/transport
variants have an explicit refusal. Unsaved Result survives refresh/language changes.

Ctrl+1/2/3/4 selects panels; Ctrl+O opens a repository; Ctrl+R opens Recent;
Ctrl+F filters paths; arrows navigate; Tab/Shift+Tab or F6 moves focus;
F5 refreshes; F7/Shift+F7 moves hunks; Shift+wheel or Alt+Left/Right/Home moves
both diff texts horizontally. Ctrl+Enter commits; Ctrl+Shift+Enter continues.
In History Ctrl+Up/Down selects changed files. Ctrl+Shift+F/P/L is Fetch/Push/Pull.

Git and OpenSSH supply the audited operation/push coordinators and existing
credentials/agent. Fedora's Vulkan loader/driver, fonts and platform libraries
supply the native display. Wayland scenarios are tested; physical input and X11
pixel/runtime certification are not claimed. Diff preview has bounded size/line
limits; source bytes are preserved. GPU execution timing is not isolated.

Build identity is in /usr/share/hunkra/build-info.json. Locked dependency declarations
and notices are in /usr/share/licenses/hunkra/third-party.json and third-party/.
These files remain installed when optional documentation is excluded.
The unmodified MPL-2.0 option-ext source files are included there as well.
Hunkra's product license is still unselected (see LICENSE-STATUS). This unsigned
RPM is a local review artifact; no publication has been performed.
