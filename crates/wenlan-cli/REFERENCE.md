# CLI runtime-bundle discovery

`wenlan doctor` and `wenlan setup` discover an app bundle for checking or refreshing `~/.wenlan/bin`.

`WENLAN_BUNDLE_DIR` optionally supplies the first bundle-discovery candidate. The value must be nonempty and point to a directory containing the platform's app marker (`wenlan-app` or `wenlan-app.exe`) and all three runnable runtime binaries (`wenlan`, `wenlan-server`, `wenlan-mcp`, with `.exe` on Windows). The home-bin directory itself is never accepted as an app bundle. An invalid candidate is skipped, and discovery continues with the current executable's directory, the recorded MCP bundle path, and platform defaults in that order.

When `WENLAN_BUNDLE_DIR` is unset or empty, discovery starts with the current executable's directory, then checks the recorded MCP path and platform defaults. `wenlan doctor` only changes `~/.wenlan/bin` when called with `--repair`; release `wenlan setup` refreshes it automatically. A debug build, or either `WENLAN_DATA_DIR` or `ORIGIN_DATA_DIR` being set, disables setup's automatic refresh. `WENLAN_BUNDLE_DIR` selects a bundle candidate only: it does not isolate `HOME` or the data directory and does not itself force a write.

## `wenlan tool` and the tool-backed commands

`wenlan tool`, the `wenlan pages read | write | rename | move | delete | history | restore | links` commands, `wenlan memories pending | confirm | forget | history` and `wenlan sources read` start a `wenlan-mcp` and speak MCP to it over stdio, so they act as an AI caller and never as the person. The binary is the `wenlan-mcp` beside the running `wenlan` (with `.exe` on Windows); `PATH` is not searched, and a missing one is an error that names the reinstall command.

`WENLAN_MCP_BIN` names the `wenlan-mcp` to start instead. It is for tests, which point it at a stand-in; when set, that path is started as given and the sibling is not looked for.
