# Sanemark logo

The logo is a rounded notebook containing an **M**, a thin underline aligned
beneath it, and a muted blue typing cursor. It uses charcoal (`#343B3E`) and
blue (`#6B80BB`); dark-surface versions use `#EEF0EE` and `#A8B7E0`.

## Assets

- `logo.svg`: the primary 256 × 256 standalone logo, with a transparent background.
- `wordmark.svg` and `wordmark-dark.svg`: the logo followed by **Sanemark** in
  DejaVu Sans Mono. The lettering is outlined as SVG paths, so viewers do not
  need the font installed. The main README selects a version using `<picture>`.
- `extension-icon.svg`: the standalone logo on a rounded, light background
  (`#F7F8F5`) so it stays legible in both light and dark extension listings.
- `../editors/vscode/images/icon.png`: the 256 × 256 export used by the VS Code
  extension manifest and its README. It contains only the logo, without a wordmark.

All final SVGs are self-contained geometry, with no text elements, scripts,
embedded bitmaps, or external resources. Preserve their proportions and padding.

## Regenerate the extension icon

From the repository root, with Chromium installed:

```sh
chromium --headless --disable-gpu --hide-scrollbars \
  --default-background-color=00000000 \
  --screenshot="$PWD/editors/vscode/images/icon.png" \
  --window-size=256,256 "file://$PWD/assets/extension-icon.svg"
```

The Marketplace requires a PNG icon; the SVG remains the editable source.
See the [VS Code publishing documentation](https://code.visualstudio.com/api/working-with-extensions/publishing-extension).
