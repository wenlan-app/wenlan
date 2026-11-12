# Setup and data: installation, models, backup, and offline queue

This guide covers installation and updates, optional model-backed background work, data locations and safe retention, and the CLI offline outbox.

Languages: [English](setup-and-data.md) · [繁體中文](setup-and-data.zh-Hant.md) · [简体中文](setup-and-data.zh-Hans.md) · [Español](setup-and-data.es-ES.md)<br>
[Back to the English README](../README.md)

Contents: [Installation](#installation) · [Models and privacy](#models) · [Backup and removal](#backup-and-removal) · [Offline queue](#offline-queue)

<a id="installation"></a>

## Installation

### Download and open Wenlan

[Download the app](https://github.com/wenlan-app/wenlan/releases/latest), then open it:

- **macOS (Apple Silicon):** open the `.dmg` and drag Wenlan to Applications. The app is signed and notarized.
- **Windows x64:** run the `-setup.exe`. It is not signed yet. If SmartScreen warns you, confirm you downloaded it from the official Releases page before choosing "More info" → "Run anyway".
- **Linux x64:** download the `.AppImage`, make it executable (`chmod +x Wenlan_*.AppImage`) and open it, or install the `.deb` on Debian and Ubuntu (`sudo apt install ./Wenlan_*_amd64.deb`). Needs Ubuntu 22.04, Debian 12 or a newer distribution with WebKitGTK 4.1. The app is not signed by an OS vendor; it updates itself like the other platforms. ARM64 Linux has no desktop build yet; follow the [setup guide](setup-with-ai.md#install-the-runtime) to use Wenlan with your AI tools without the app.

First launch downloads a model for local search, so keep an internet connection until setup finishes. [Download and privacy details](PRIVACY.md#when-wenlan-reaches-the-network).

### Other installation options and updates

Optional background organization and automatic Page updates need a [configured model](#models); this is separate from the search model downloaded at first launch.

**macOS app from the terminal**

The installer downloads the app, checks its SHA-256, and moves it to Applications:

```bash
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/wenlan-app/wenlan/main/scripts/install-macos-app.sh)"
```

**Without the desktop app**

On macOS Apple Silicon:

```bash
npx -y wenlan setup
```

`npx` requires Node.js; without it, run `curl -fsSL https://raw.githubusercontent.com/wenlan-app/wenlan/main/install.sh | bash` then `wenlan setup --basic`.

This downloads the prebuilt CLI, daemon, and MCP connector, starts the local runtime, and verifies it. No Rust toolchain or Cargo is required. Linux x64/ARM64 with glibc has an automated [shell setup path](setup-with-ai.md#install-the-runtime); Windows x64 uses the matching archive from [Releases](https://github.com/wenlan-app/wenlan/releases/latest). macOS Intel currently has [no supported complete-runtime install](../crates/wenlan-cli/README.md#macos-intel).

**What is installed and how to update**

The desktop app bundles the daemon, CLI, and MCP connector. It starts the daemon on launch and offers to connect detected AI clients through the Claude Code or Codex plugin, or an MCP entry for other supported clients. The headless install runs the same daemon without a window; either way, your AI tools use the same local knowledge base.

To upgrade the macOS app, drag the new app over the old one and open it. Quit Wenlan 0.17.0 and older by hand first.

Manual and client-specific instructions: [AI-assisted setup](setup-with-ai.md) · [Claude Code plugin](../plugin/README.md) · [Codex plugin](../plugin-codex/README.md) · [CLI and MCP](../crates/wenlan-cli/README.md).

<a id="models"></a>

## Models and privacy

Use your connected AI to write pages without installing an additional local language model. Optional background organization and automatic updates need a separately configured model.

- **Search stays local.** The search model downloads at first launch and runs on your machine, without an API key.
- **Connected AI can receive your knowledge.** A cloud AI client can send retrieved content to its provider. Choosing a cloud model for background organization also sends the material needed for that task. Local storage does not make those interactions local.
- **Usage statistics are off by default.** If you opt in, Wenlan sends limited operation counts, version, and platform, not your knowledge content or an installation ID. [Details](PRIVACY.md#telemetry).

See [network and privacy details](PRIVACY.md#when-wenlan-reaches-the-network) for downloads, update checks, remote images, and optional remote access.

### Model choices and technical details

- **Local base retrieval:** The [BGE embedding model](https://huggingface.co/Qdrant/bge-base-en-v1.5-onnx-Q) runs through FastEmbed on your machine for hybrid search and needs no API key.
- **Optional on-device synthesis:** Steep (background enrichment) and Page synthesis can use user-selected [`Qwen3.5 4B`](https://huggingface.co/unsloth/Qwen3.5-4B-GGUF) or [`Qwen3.5 9B`](https://huggingface.co/unsloth/Qwen3.5-9B-GGUF) through [llama.cpp](https://github.com/ggml-org/llama.cpp). Wenlan does not download or activate a language model until you choose one. `wenlan steep status` shows whether Steep is off, ready, or paused; `wenlan steep configure` turns it on.
- **Other providers:** An OpenAI-compatible local endpoint such as Ollama or LM Studio, or a configured cloud provider, can supply model-backed Steep and synthesis instead.
- **MLX on a Mac:** To run a model in Apple's MLX format, load it in LM Studio or Ollama and choose that provider in Wenlan settings. Steep, synthesis and the sentence-by-sentence source check then use it.
- **Cloud disclosure:** If the model endpoint you select is remote, Wenlan sends that task's system and user prompts to it. Local retrieval and on-device synthesis stay on your machine.

Full workflow reference: [plugin/skills](../plugin/skills/README.md). Technical model roles: [technical foundations](technical-foundations.md#model-roles).

<a id="backup-and-removal"></a>

## Backup and removal

Your pages and session notes are Markdown files; memories and the graph live in a local database. You can keep your knowledge when you uninstall the app.

### Default locations

- Pages and session notes: `~/.wenlan/`.
- Database and runtime data: `~/Library/Application Support/wenlan/` on macOS, `~/.local/share/wenlan/` on Linux, or `%LOCALAPPDATA%\wenlan\` on Windows.
- Your data as files: `files-mirror/` in the database folder. Every change is written to the write log there (`files-mirror/log/`) before the database takes it, and the readable files in `files-mirror/files/` follow within a few minutes. The database is only an index of these files: if the database file is missing or empty when Wenlan starts, Wenlan rebuilds it from the files and the log, the last changes included, then checks the pages against their sources again in the background. The check reuses the AI's earlier judgments, so text that has not changed needs no new ones. Once a day Wenlan also checks that the files rebuild the same database. Do not delete this folder. `WENLAN_FILES_MIRROR=0` turns it off; the database is then written alone, as in earlier versions.

### Backing up your knowledge

Quit the app and stop the background service before copying these folders. Include any custom page or data folders you configured. A wiki export is not a full database backup.

Wenlan does not put `~/.wenlan` under version control. Page history lives in Wenlan's own revisions (`wenlan pages "<query>" --print` shows a page with its sources). If you also want a git history of the Markdown files, run `git init` in `~/.wenlan/pages` yourself and commit when you like; Wenlan never touches that repository.

If you upgraded from Origin, also check `~/.origin/` and the sibling `origin` platform data folder for older copies. Include those when backing up old material, or remove them only if you intend to delete it.

### Uninstalling

Turn off *Run Wenlan in background at login* in Settings, quit the app, then delete `Wenlan.app` or run the Windows uninstaller. Your knowledge folders remain; delete them only if you no longer want the data and have any backup you need.

`wenlan background off` stops the daemon and disables autostart, but does not remove the service registration. CLI-only uninstall, leftover AI-client settings, credentials, and other stored files are covered in the [removal details](PRIVACY.md#data-deletion).

<a id="offline-queue"></a>

## Offline queue (outbox)

If the local daemon is unreachable, `wenlan capture` and `wenlan brief update` write their requests to a durable local outbox and exit successfully. When the daemon returns, it drains those writes through the normal HTTP routes; inspect the queue with `wenlan outbox status` or request an immediate replay with `wenlan outbox drain`. A write the daemon rejects outright (a 4xx, such as failing the content quality gate) moves to `outbox/failed/` with a receipt instead of retrying forever; a transport failure or server error (5xx) leaves it queued for the next drain, which runs automatically every 60 seconds.
