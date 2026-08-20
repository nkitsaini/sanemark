# Release Process for sanemark

This document describes the steps required to publish a new release of `sanemark`.

---

## Pre-release Checklist

Before creating a release, ensure all local tests, lints, and flake checks pass cleanly:

```bash
# 1. Check formatting
cargo fmt --all -- --check

# 2. Run Clippy (deny warnings)
cargo clippy --all-targets --all-features -- -D warnings

# 3. Run all unit, CLI, formatting, and LSP integration tests
cargo test --all-targets --all-features

# 4. Validate Nix packaging
nix flake check
```

---

## Release Steps

### 1. Bump the Version

Update the version number in `Cargo.toml`:

```toml
[package]
name = "sanemark"
version = "0.1.0" # -> bump to target version (e.g. 0.2.0)
```

Update `Cargo.lock` by running:

```bash
cargo check
```

### 2. Commit Version Bump

```bash
git add Cargo.toml Cargo.lock
git commit -m "chore: release v0.1.0"
git push origin main
```

### 3. Create and Push Git Tag

Create an annotated tag matching the version:

```bash
git tag -a v0.1.0 -m "Release v0.1.0"
git push origin v0.1.0
```

---

## Automated GitHub Release Pipeline

Pushing a `v*` tag automatically triggers the [Release Workflow](.github/workflows/release.yml), which:

1. Cross-compiles optimized binaries for:
   - **Linux**: `x86_64-unknown-linux-gnu`, `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-gnu`, `aarch64-unknown-linux-musl`
   - **macOS**: `x86_64-apple-darwin` (Intel), `aarch64-apple-darwin` (Apple Silicon)
   - **Windows**: `x86_64-pc-windows-msvc`, `aarch64-pc-windows-msvc`
2. Bundles the `sanemark` binary (and `sanemark-lsp` alias), `README.md`, and `LICENSE` into `.tar.gz` (Unix) and `.zip` (Windows) archives.
3. Computes SHA256 checksums (`.sha256`).
4. Creates a GitHub Release and attaches all binary archives and checksums.

---

## Publish to Crates.io

Once the GitHub Release succeeds, publish the crate to [crates.io](https://crates.io):

```bash
cargo publish
```

---

## Post-Release Verification

1. **Verify one-line installer**:
   ```bash
   curl -fsSL https://raw.githubusercontent.com/nkitsaini/sanemark/main/install.sh | sh
   sanemark --version
   ```
2. **Verify Nix run**:
   ```bash
   nix run github:nkitsaini/sanemark -- --version
   ```
3. **Verify `cargo binstall` / `cargo install`**:
   ```bash
   cargo binstall sanemark
   # or
   cargo install sanemark
   ```
