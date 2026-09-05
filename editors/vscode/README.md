# Sanemark for Visual Studio Code

[Sanemark](https://github.com/nkitsaini/sanemark) is a fast, zero-config Markdown language server and toolchain written in Rust.

This extension brings the full power of Sanemark to VS Code: reference-link management, table and list formatting, link completions, broken-link diagnostics, and daily journal notes.

---

## Features

- **Consolidated Reference Links**: Automatically collect all inline links into a clean `## References` section at the bottom of the document upon formatting.
- **Convert & Inline Commands**:
  - `Sanemark: Move References to Bottom`: Consolidate all links into reference definitions.
  - `Sanemark: Inline References`: Inline all reference definitions back into text.
  - `Sanemark: Convert to Inline Link`: Convert selected reference links into inline links.
  - `Sanemark: Copy as Inlined Markdown`: Copy the current document or selection with all references converted to inline links to your system clipboard without modifying your file.
- **GFM Table Normalization**: Formats and neatly aligns GitHub Flavored Markdown tables with proper delimiter rows.
- **List Normalization**: Normalizes list markers to `-` and renumbers ordered lists incrementally.
- **Broken Link Diagnostics**: Catches broken relative file links and images directly in the editor.
- **Fast Path Completion**: Offers auto-completions for relative paths, images, and Markdown notes with fuzzy matching.
- **Quick-Insert Snippets**: Type `/` or `@` at the start of a line or after a space to insert dates (`today`, `tomorrow`, `yesterday`), timestamps, or linked workspace files.
- **Journal & Daily Notes**: Quickly open or create daily notes from configurable templates (`Sanemark: Open Today's Journal Note`).

---

## Getting Started

### Automatic Binary Setup
By default, the extension will check if `sanemark` is available on your `$PATH`. If not found, it prompts you to automatically download the latest pre-compiled binary release for your platform directly from GitHub Releases.

### Manual Setup
If you prefer to compile or manage the binary yourself:

```bash
# Using cargo
cargo install sanemark

# Or using Nix
nix profile install github:nkitsaini/sanemark
```

You can also explicitly configure the path to the executable in your VS Code settings:

```json
{
  "sanemark.serverPath": "/path/to/sanemark"
}
```

To see which executable the extension selected, open **View: Toggle Output** from
the Command Palette and choose **Sanemark** in the Output panel's channel picker.

---

## Extension Commands

All commands can be found in the Command Palette (`Ctrl+Shift+P` / `Cmd+Shift+P`):

| Command | Description |
| :--- | :--- |
| `Sanemark: Move References to Bottom` | Rewrites inline links as reference definitions at the end of the file. |
| `Sanemark: Inline References` | Converts reference links back to inline links in the entire document. |
| `Sanemark: Convert to Inline Link` | Converts reference links in the active selection to inline links. |
| `Sanemark: Copy as Inlined Markdown` | Copies the selection or document to the clipboard with references inlined. |
| `Sanemark: Open Today's Journal Note` | Opens or creates today's daily journal note. |
| `Sanemark: Open Yesterday's Journal Note` | Opens yesterday's daily journal note. |
| `Sanemark: Open Tomorrow's Journal Note` | Opens tomorrow's daily journal note. |
| `Sanemark: Restart Language Server` | Restarts the background Sanemark language server process. |
| `Sanemark: Download or Update Language Server` | Downloads the latest Sanemark binary release for your system. |

---

## Configuration

Customise behaviour in your VS Code settings (`settings.json`):

```json
{
  // Path to custom sanemark executable (null = PATH or auto-downloaded)
  "sanemark.serverPath": null,

  // GitHub Flavored Markdown support
  "sanemark.gfm": true,

  // Formatting options
  "sanemark.formatting.moveReferencesToBottom": true,
  "sanemark.formatting.referencesHeading": "References",
  "sanemark.formatting.formatTables": true,
  "sanemark.formatting.formatLists": true,

  // Diagnostics
  "sanemark.diagnostics.brokenLinks": true,
  "sanemark.diagnostics.severity": "warning",
  "sanemark.diagnostics.checkImages": true,
  "sanemark.diagnostics.ignore": [],

  // Completions
  "sanemark.completion.paths": true,
  "sanemark.completion.pathStyle": "auto",
  "sanemark.completion.showHiddenFiles": false,
  "sanemark.completion.gitignore": true,
  "sanemark.completion.deepPaths": true,

  // Snippets
  "sanemark.snippets.enabled": true,
  "sanemark.snippets.fileLinks": true,
  "sanemark.snippets.dateFormat": "%Y-%m-%d",
  "sanemark.snippets.timeFormat": "%H:%M",

  // Journal / Daily Notes
  "sanemark.journal.directory": "journal",
  "sanemark.journal.template": null
}
```

---

## License

MIT © [Ankit Saini](https://github.com/nkitsaini)
