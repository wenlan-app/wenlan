# Bring existing material into Wenlan

[English](import-guide.md) · [繁體中文](import-guide.zh-Hant.md) · [Back to README](../README.md#get-started)

Open **Settings → Sources** and choose the route that matches your material.

## Connect an Obsidian vault — read-only

1. Under **Connected sources**, choose **Add source…** (or **Add your first source**).
2. Browse to your vault folder. Wenlan detects the vault; choose **Connect** to start indexing.
3. After changing notes in Obsidian, use **Sync** beside the source to refresh Wenlan’s index.

Wenlan reads the vault’s Markdown notes without changing the original files. **Sync reads changes into Wenlan; it does not write changes back to Obsidian.** Keep editing your vault in Obsidian.

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
