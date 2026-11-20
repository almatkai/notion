# Notion for Tezbar

Native Rust extension for Tezbar 0.3.0-beta.12 and later. The Rust process serves six GPUI commands. See [PORT-REPORT.md](PORT-REPORT.md) for coverage and validation.

## Connect

1. Create a Notion internal integration with read, insert and update content capabilities appropriate for the commands you plan to use.
2. In Notion, connect each page or database you want the extension to access to that integration.
3. Open the Notion extension preferences in Tezbar and enter **Internal Integration Secret**. Run **Manage Notion Connection** and select **Test Connection**.

[Notion connections and tokens](https://developers.notion.com/docs/authorization)

Use **Search Notion** to find pages and databases. Select a result, then open its actions to preview, append content, edit properties or browse its data source. Choose **Find by URL or ID** to open a known page or pick a data source from a database with several sources.

**Add Note** files text under the configured notes root and daily page. Select **Choose Parent Page** when creating the notes root for the first time. **Quick Capture** saves a bookmark, readable article text, or an AI summary. AI summaries use the provider configured in Tezbar after you confirm the transfer.

## Local validation

Run `python scripts/verify-local.py` from this directory. It uses the installed Rust and Python toolchains and builds locally. Protocol tests authenticate only to their owned loopback server with synthetic data. The generated target ZIP is a local development package.
