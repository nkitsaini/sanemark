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

## Standard Stable Release

For regular iterative releases (`v0.1.0`, `v0.1.1`, `v0.2.0`), publish directly:

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

### 2. Commit and Tag

```bash
git add Cargo.toml Cargo.lock
git commit -m "chore: release v0.1.0"
git push origin main

git tag -a v0.1.0 -m "Release v0.1.0"
git push origin v0.1.0
```

---

## Release Candidate (RC) Approach

Release Candidates (`-rc.1`, `-rc.2`, `-beta.1`) are recommended before major milestones (such as `v1.0.0` or major breaking changes) to test across OSes and editor clients before freezing stable versions.

### 1. Set the RC Version

Update `Cargo.toml`:

```toml
[package]
name = "sanemark"
version = "1.0.0-rc.1"
```

Update `Cargo.lock`:

```bash
cargo check
```

### 2. Commit and Tag the RC

```bash
git add Cargo.toml Cargo.lock
git commit -m "chore: release v1.0.0-rc.1"
git push origin main

git tag -a v1.0.0-rc.1 -m "Release v1.0.0-rc.1"
git push origin v1.0.0-rc.1
```

### 3. How the Ecosystem Handles Pre-releases

- **GitHub Releases**: The release workflow automatically detects the pre-release tag and marks the release as **`Pre-release`** (it does **not** become the `Latest` release).
- **One-line installer (`install.sh`)**: Continues to serve the latest stable release; it will **not** serve pre-releases to ordinary users.
- **Crates.io**: You can publish pre-releases (`cargo publish`). Cargo will ignore pre-releases by default; users only receive it if they explicitly specify `--version 1.0.0-rc.1`.
- **Package Managers & Editor Registries**: Homebrew, AUR, Nixpkgs, and Mason ignore pre-releases and track only stable tags.

### 4. Graduating from RC to Final Release

Once testing is complete, bump `version = "1.0.0"` in `Cargo.toml`, commit, and tag `v1.0.0`.

---

## Automated GitHub Release Pipeline

Pushing any `v*` tag automatically triggers the [Release Workflow](.github/workflows/release.yml), which:

1. Cross-compiles optimized binaries for:
   - **Linux**: `x86_64-unknown-linux-gnu`, `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-gnu`, `aarch64-unknown-linux-musl`
   - **macOS**: `x86_64-apple-darwin` (Intel), `aarch64-apple-darwin` (Apple Silicon)
   - **Windows**: `x86_64-pc-windows-msvc`, `aarch64-pc-windows-msvc`
2. Bundles the `sanemark` binary (and `sanemark-lsp` alias), `README.md`, and `LICENSE` into `.tar.gz` (Unix) and `.zip` (Windows) archives.
3. Computes SHA256 checksums (`.sha256`).
4. Creates a GitHub Release (or Pre-release) and attaches all binary archives and checksums.

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
