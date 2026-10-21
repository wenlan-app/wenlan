# Bring existing material into Wenlan

[English](import-guide.md) · [繁體中文](import-guide.zh-Hant.md) · [Back to README](../README.md#get-started)

Open **Settings → Sources** and choose the route that matches your material.

## Connect an Obsidian vault — read-only

1. Under **Connected sources**, choose **Add source…** (or **Add your first source**).
2. Browse to your vault folder. Wenlan detects the vault; choose **Connect** to start indexing.
3. Keep editing in Obsidian. While Wenlan runs, it checks the vault for changes about every 30 seconds: edited, new, renamed, and deleted notes. To pick up a change right away, use **Sync** beside the source.

Wenlan reads the vault’s Markdown notes without changing the original files. **Changes flow one way, into Wenlan; nothing is written back to Obsidian.**

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

If your AI tool can read Gmail, Notion, Google Drive, Slack, GitHub, or the web, ask it to keep what it read in Wenlan, for example "save this email to Wenlan". With the Wenlan plugin connected, the tool saves the text together with a link to the original, so notes that cite it open the email or page it came from. You can also save one from the command line: `wenlan sources save --url <link> --title <title> --label gmail < email.txt`.

Wenlan keeps the copy that was saved and does not fetch the original again. When the original changes, ask your tool to save it again with the same link; Wenlan replaces the copy and marks the notes that rely on it for refresh. This works from a local tool such as Claude Code or Codex. Web access for claude.ai and ChatGPT is read-only, so it cannot save sources.
