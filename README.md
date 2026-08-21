


# sanemark

A markdown language server that implements folding, table formatting, file linking and moving inline links to references in a sane way.

I have been daily driving sanemark for my own notes for a while now.




https://github.com/user-attachments/assets/7e4f892f-30bb-432b-a20a-ed3b7b881a63

  
## What it does

### Standard Markdown links

Sanemark completes paths inside ordinary links:

- Fuzzy, workspace-wide file search
- Relative and workspace-root paths
- Go to definition and find references
- Diagnostics for broken local links

Path completion respects `.gitignore` and works without project configuration.

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

Zed currently requires using a recognized language-server key. Add the following
to your project or user settings, replacing the path with an absolute path to
the binary:

```jsonc
{
  "languages": {
    "Markdown": {
      "language_servers": ["typescript-language-server"],
      "format_on_save": "on",
      "formatter": {
        "language_server": {
          "name": "typescript-language-server"
        }
      }
    }
  },
  "lsp": {
    "typescript-language-server": {
      "binary": {
        "path": "/absolute/path/to/sanemark",
        "arguments": []
      }
    }
  }
}
```

This repurposes an unused adapter key for Markdown. It does not install or run
the TypeScript language server.

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

## License

[MIT](LICENSE)
