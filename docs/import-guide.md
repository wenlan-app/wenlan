# Bring existing material into Wenlan

[English](import-guide.md) · [繁體中文](import-guide.zh-Hant.md) · [Back to README](../README.md#get-started)

Open **Settings → Sources** and choose the route that matches your material.

## Connect an Obsidian vault

1. Under **Connected sources**, choose **Add source…** (or **Add your first source**), then **Connect Obsidian vault**.
2. Browse to your vault folder.
3. Choose **Keep in sync** or **Import once**:
   - **Keep in sync** (the default): keep editing in Obsidian. While Wenlan runs, it checks the vault for changes about every 30 seconds: edited, new, renamed, and deleted notes.
   - **Import once**: Wenlan reads the vault now. Later changes wait until you choose **Refresh** in the source's info.

   You can switch later with **Keep in sync** in the source's info. To pick up a change right away, choose **Refresh** there.

Wenlan reads the vault’s Markdown notes and **never edits them without your OK**. The vault, each folder, and each note have an **AI changes** setting in the source's info:

- **Never** (where your notes start): AI only cites the note. PDFs are always Never.
- **Ask me first**: AI can suggest a change. Notes an AI wants to change appear on the home and in the menu bar, and the file changes only when you choose **Accept**.

A folder or a note can have its own setting; the nearest one wins.

Wenlan's own pages can live in the vault too. The source's info shows where they are and offers to move them into the vault, so they open in Obsidian next to your notes. Wenlan finishes the move when it reopens.

## Connect a Logseq graph — read-only

1. Choose **Add source…**, then **Connect Logseq graph**, and pick the graph folder (the one that holds `pages/`, `journals/`, and `logseq/`).
2. Choose **Keep in sync** or **Import once**, as for an Obsidian vault. Kept in sync, Wenlan follows edits, new pages, renames, and deletions about every 30 seconds while you keep writing in Logseq.

Wenlan reads the Markdown pages in `pages/` and `journals/`. Outlines become plain paragraphs and lists, a journal is titled with its date, and block properties such as `id::` are left out. Logseq's backups in `logseq/bak/` are not read, and nothing is written back to Logseq. Graphs made with the newer Logseq database version keep their pages in a database, not in files: export them to Markdown first.

## Import a ChatGPT or Claude conversation ZIP

1. Request an export from your AI account:
   - **ChatGPT:** Settings → Data controls → Export data. [Official instructions](https://help.openai.com/en/articles/7260999-exporting-your-chatgpt-history-and-data).
   - **Claude:** Settings → Privacy → Export data. [Official instructions](https://support.claude.com/en/articles/9450526-export-your-claude-data).
2. Download the export, then drop the ZIP into **Import chat history**, or choose the file there. You do not need to unzip it.
3. Check the import result. Previously imported conversations are skipped; further processing may continue in the background.

This imports paired user and assistant text, not attachments or other multimodal content. If Claude provides a manifest JSON, download the conversation ZIPs listed in it and import each ZIP; the manifest itself cannot be imported. This is a snapshot import, not a live connection to your AI account.

## Import existing memories as text

1. Under **Import Memories**, choose **Paste or import**.
2. Select ChatGPT, Claude, or Other. Paste the memories you want to keep, **one memory per line**. If needed, use the prompt provided in the form to ask your AI for an export of its memories.
3. Review the text, choose **Import**, then open **View memories** to check what was saved.

Use this for concise preferences, decisions, facts, or lessons you already have. Use the ZIP route above for conversation history.

## Save an email, page, or thread through your AI tool

If your AI tool can read Gmail, Notion, Google Drive, Slack, GitHub, or the web, ask it to keep what it read in Wenlan, for example "save this email to Wenlan". With the Wenlan plugin connected, the tool saves the text together with a link to the original, so pages that cite it open the email or page it came from. You can also save one from the command line: `wenlan sources save --url <link> --title <title> --label gmail < email.txt`.

Wenlan keeps the copy that was saved and does not fetch the original again. When the original changes, ask your tool to save it again with the same link; Wenlan replaces the copy and marks the pages that rely on it for refresh. This works from a local tool such as Claude Code or Codex. Web access for claude.ai and ChatGPT cannot save sources, even for an app you allowed to write.
