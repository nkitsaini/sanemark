# sanemark

A markdown language server that focuses on easy note-taking in IDEs, although things like table formatting make it good for non-note related tasks as well.

Demo Video: https://github.com/user-attachments/assets/7e4f892f-30bb-432b-a20a-ed3b7b881a63

I have been daily driving sanemark for my own notes for a while now.

## Contents

- [What sets it apart?](#what-sets-it-apart)
- [Getting started](#getting-started)
- [Editor setup](#editor-setup): [VS Code / Cursor](#vs-code-cursor-and-compatible-editors), [Zed](#zed), [Neovim](#neovim)
- [Installation](#installation) (standalone binary)
- [Command-line usage](#command-line-usage)
- [Configuration](#configuration)
- [Development](#development)
- [License](#license)

## What sets it apart?

### Usability for note taking

These features make it easier to use markdown for note-taking without using a WYSIWYG editor or having a "Preview" pane open on the side.

#### Cleaner inline links

Generally when taking notes, I find the inline links very distracting. For example:
```md
- Read [Oxide RFD](https://rfd.shared.oxide.computer/rfd/0063) today
```

The link makes it very hard to read full sentence. This LSP implements a way to format these links to markdown references.

```md
- Read [Oxide RFD][1] today

# References

[1]: https://rfd.shared.oxide.computer/rfd/0063
```

I personally find this much more readable and go to definition continues to work. Code actions allow you to convert references to inline links in case you need to copy/paste the content.

- Copy a selection with all references expanded using code action
- Run the same transformations from the CLI

Reference definitions are resolved across the whole document, so copied
selections remain self-contained.

#### Standard file linking

One more thing that irks me about existing status-quo is that file-linking is non-standard. Most Markdown LSPs either do not provide file completion or use non-standard syntax, most of the time the one used by Obsidian.

It looks like this:
```md

# What most other LSPs do
[[./hello.md]]

# What this LSP does
[hello](./hello.md)
```

A nice benefit of the standard relative-link syntax shown above is that these links also work on GitHub and most other platforms. This combined with `References` makes it readable as well.

Auto complete can be triggered via `@` or `/` by default.

Similarly I have found the path resolution of LSPs I have tried confusing. This LSP uses document-relative paths or workspace-root paths written with a leading `/` (not the filesystem-absolute paths!).

By default, `auto` uses a hybrid style in Git worktrees: relative paths for siblings and children, and workspace-root paths for other files. Outside Git worktrees, it normally uses relative paths. GitHub works well with both of these.

- Fuzzy, workspace-wide file search
- Go to definition and find references
- Diagnostics for broken local links, with fuzzy quick fixes for misspelled or moved files (up to five suggestions; preserves anchors and queries)

Path completion respects `.gitignore` and works without project configuration.
File completions and broken-link quick fixes also work in reference definitions
(`[label]: ./path`), wherever they appear; no special heading is required.

#### Daily notes

You can use "Open today's journal note" codeaction to create a daily note. Codeactions on most IDEs can be triggered using `ctrl+.`.

Daily notes support optional templates.

#### Quick insertion

Typing `@` or `/` lets you quickly insert dates, times, and links to files in
your workspace. This keeps common note-taking actions close at hand while still
using standard Markdown links.

### The baseline for a good Markdown LSP

Sanemark formats GFM tables, normalizes list markers and numbering, organizes
reference definitions, and provides document outlines and folding. Formatting
is available through LSP clients and the command line.

#### Efficiency

With my notes folder containing more than 400 files, more than 110k words and 20 open tabs, the LSP used less than 10 MB of RAM.

I haven't tested performance of file completion and formatting, but in my day to day all operations have been instantaneous.

Sanemark applies incremental document updates and caches parsed analysis between
requests.

#### Reliability

I don't want random bugs popping up when I am in the zone. I haven't seen a single crash in my usage till now and editing my notes have become a more pleasant experience due to this LSP. Please file github issues if you discover any bugs.

## Getting started

1. Set up your editor using the instructions below, then open a Markdown file.
2. Type `@` or `/` to insert file links, dates, and times.
3. Run your editor's **Format Document** action to format tables and lists and move inline links to reference definitions.
4. Use code actions to convert reference links back to inline links or **Open today's journal note**.

No configuration is required by default. See [Configuration](#configuration) to customize formatting, completion, and daily notes.

## Editor setup

Start with the extension for your editor. The VS Code and Zed extensions can
download the language server for you; a separate installation is usually unnecessary.
For other LSP clients, install the [standalone binary](#installation) and configure
your editor to run `sanemark` for Markdown files over stdio.

### VS Code, Cursor, and compatible editors

Install **Sanemark Markdown LSP** from the [Visual Studio Marketplace](https://marketplace.visualstudio.com/items?itemName=nkit.sanemark) or [Open VSX](https://open-vsx.org/extension/nkit/sanemark).

In VS Code, you can also open Quick Open (`Ctrl+P` / `Cmd+P` on macOS), paste the following command, and press Enter:

```text
ext install nkit.sanemark
```

Open a Markdown file and accept the download prompt if the server is not already
installed. See the [VS Code extension guide](editors/vscode/README.md) for commands
and extension settings.

### Zed

Install the Sanemark extension from Zed's extension gallery [once it is
published](https://github.com/zed-industries/extensions/pull/7344). In the meanwhile you can use the workaround documented in this [Github discussion](https://github.com/zed-industries/zed/discussions/24092#discussioncomment-15278796).

The extension uses an existing `sanemark` binary on `PATH` or downloads the appropriate binary from the latest
GitHub release automatically.

To use Sanemark for Markdown formatting, add this to your project or user
settings:

```jsonc
{
  "languages": {
    "Markdown": {
      "language_servers": ["sanemark"],
      "format_on_save": "on",
      "formatter": {
        "language_server": {
          "name": "sanemark"
        }
      }
    }
  }
}
```

<details>
<summary>Advanced: custom binary and initialization options</summary>

You can override the downloaded or `PATH` binary and pass initialization
options through Zed's LSP settings:

```jsonc
{
  "lsp": {
    "sanemark": {
      "binary": {
        "path": "/absolute/path/to/sanemark",
        "arguments": []
      },
      "initialization_options": {
        "formatting": {
          "formatTables": true,
          "formatLists": true,
          "moveReferencesToBottom": true
        }
      }
    }
  }
}
```

</details>

### Neovim

Install the [standalone binary](#installation) first, then configure Neovim's built-in LSP:

```lua
vim.lsp.config.sanemark = {
  cmd = { "sanemark" },
  filetypes = { "markdown" },
  root_markers = { ".sanemark.json", ".sanemark.jsonc", ".git" },
}

vim.lsp.enable("sanemark")
```

## Installation

Install the standalone binary for [command-line use](#command-line-usage), Neovim,
or another editor without automatic server downloads. If your editor extension
manages the server, you can skip this section.

### Installer

On Linux and macOS:

```bash
curl -fsSL https://raw.githubusercontent.com/nkitsaini/sanemark/main/install.sh | sh
```

### Cargo

```bash
cargo install sanemark
```

### Nix

```bash
nix run github:nkitsaini/sanemark
nix profile install github:nkitsaini/sanemark
```

## Command-line usage

The same binary works as an LSP server and a command-line tool. Running
`sanemark` without a subcommand starts the language server over stdio.
See `sanemark --help` for all options.

```bash
# Format one file, or every Markdown file in a directory.
sanemark format README.md
sanemark format --write .
sanemark format --check docs/

# Expand reference links and print the result.
sanemark inline notes.md
sanemark inline notes.md | wl-copy

# Report links and images whose local targets do not exist.
sanemark lint .

# Print the complete default configuration.
sanemark config
```

Directory commands search recursively for `.md` and `.markdown` files. They
respect `.gitignore` and skip hidden files by default.

This makes it usable in Git Hooks and CI.

## Configuration

Sanemark has defaults for every option. Run `sanemark config` to print a
full commented configuration file containing the options supported by your installed
version.

Settings can come from your editor's LSP initialization options or from
`.sanemark.json` / `.sanemark.jsonc` in the workspace root:

```jsonc
{
  "completion": {
    "pathStyle": "auto",
    "prioritizeExtensions": [".md", ".markdown"]
  },
  "formatting": {
    "formatTables": true,
    "formatLists": true,
    "moveReferencesToBottom": true
  },
  "journal": {
    "directory": "journal",
    "template": "journal/template.md"
  }
}
```

Project settings override editor settings, making the same conventions portable
across editors and the CLI.

## Development

```bash
cargo test
```

Benchmarks cover parsing, formatting, completion, diagnostics, and
incremental edits:

```bash
cargo bench
```

To enter the Nix development shell:

```bash
nix develop
```

To check the Zed extension adapter:

```bash
rustup target add wasm32-wasip2
cargo check --manifest-path editors/zed/Cargo.toml --target wasm32-wasip2
```

## License

[MIT](LICENSE)
