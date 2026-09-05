# sanemark

A markdown language server that focuses on easy note-taking in IDEs, although things like table formatting make it good for non-note related tasks as well.

Demo Video: https://github.com/user-attachments/assets/7e4f892f-30bb-432b-a20a-ed3b7b881a63

I have been daily driving sanemark for my own notes for a while now.

## What sets it apart?

### 1. Efficiency

With my notes folder containing more than 400 files, more than 110k words and 20 open tabs, the LSP used less than 10 MB of RAM.

I haven't tested performance of file completion and formatting, but in my day to day all operations have been instantaneous.

### 2. Usability for note taking

These features make it easier to use markdown for note-taking without using a WYSIWYG editor or having a "Preview" pane open on the side.

#### 2.a Cleaner inline links
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


#### 2.b Standard file linking

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

#### 2.c Daily notes

You can use "Open today's journal note" codeaction to create a daily note. Codeactions on most IDEs can be triggered using `ctrl+.`.

### 3. The basics I expect from a Markdown LSP

The LSP does not require a config, but allows you to configure various aspects. Run `sanemark config` to see all options.

It handles markdown table formatting and can provide fold information to IDEs as well.


### 4. Has a CLI interface as well

You can use `sanemark format` to format files and `sanemark lint` to find links to files that don't exist. See `sanemark --help` for all options.

This makes it usable in Git Hooks and CI.

### 5. Reliability

I don't want random bugs popping up when I am in the zone. I haven't seen a single crash in my usage till now and editing my notes have become a more pleasant experience due to this LSP. Please file github issues if you discover any bugs.

## What it does

### Standard Markdown links

Sanemark completes paths inside ordinary links:

- Fuzzy, workspace-wide file search
- Relative and workspace-root paths
- Go to definition and find references
- Diagnostics for broken local links, with fuzzy quick fixes for misspelled or moved files (up to five suggestions; preserves anchors and queries)

Path completion respects `.gitignore` and works without project configuration.
File completions and broken-link quick fixes also work in reference definitions
(`[label]: ./path`), wherever they appear; no special heading is required.

### Reference links without the friction

Keep long URLs and file paths out of the reading flow by moving link definitions
to the bottom of the document:

- Convert inline links to reference links
- Convert reference links back to inline links using code action
- Copy a selection with all references expanded using code action
- Run the same transformations from the CLI

Reference definitions are resolved across the whole document, so copied
selections remain self-contained.

### Formatting that stays out of the way

Sanemark formats GFM tables, normalizes list markers and numbering, and can
organize reference definitions. Formatting is available through LSP clients
and the command line.

### Useful editing features

- Document outline and folding
- Quick date, time, and file-link insertion with `@` or `/`
- Daily notes with optional templates
- Incremental document updates and cached analysis

## Command-line usage

The same binary works as an LSP server and a command-line tool. Running
`sanemark` without a subcommand starts the language server over stdio.

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

## Configuration

Sanemark has defaults for every option. Run `sanemark config` to print a
commented configuration file containing the options supported by your installed
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

## Installation

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

## Editor setup

Sanemark speaks standard LSP over stdio. Configure your editor to run the
`sanemark` binary for Markdown files.

### VS Code, Cursor, and compatible editors

Install **Sanemark Markdown LSP** from the [Visual Studio Marketplace](https://marketplace.visualstudio.com/items?itemName=nkit.sanemark) or [Open VSX](https://open-vsx.org/extension/nkit/sanemark).

In VS Code, you can also open Quick Open (`Ctrl+P` / `Cmd+P` on macOS), paste the following command, and press Enter:

```text
ext install nkit.sanemark
```

### Neovim

With Neovim's built-in LSP:

```lua
vim.lsp.config.sanemark = {
  cmd = { "sanemark" },
  filetypes = { "markdown" },
  root_markers = { ".sanemark.json", ".sanemark.jsonc", ".git" },
}

vim.lsp.enable("sanemark")
```

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

## Performance

Sanemark applies incremental document updates and caches parsed analysis between
requests. Benchmarks cover parsing, formatting, completion, diagnostics, and
incremental edits:

```bash
cargo bench
```

## Development

```bash
cargo test
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
