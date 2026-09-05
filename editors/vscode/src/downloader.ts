import * as fs from "fs";
import * as path from "path";
import * as os from "os";
import { execFile } from "child_process";
import { promisify } from "util";
import * as vscode from "vscode";

const execFileAsync = promisify(execFile);
const REPO = "nkitsaini/sanemark";

export interface Release {
  tag_name: string;
  assets: Array<{ name: string; browser_download_url: string }>;
}

export async function fetchLatestRelease(): Promise<Release> {
  const res = await fetch(`https://api.github.com/repos/${REPO}/releases/latest`, {
    headers: { "User-Agent": "vscode-sanemark" },
    signal: AbortSignal.timeout(30_000),
  });
  if (!res.ok) throw new Error(`GitHub API returned ${res.status}: ${res.statusText}`);
  return await res.json() as Release;
}

export function managedBinary(context: vscode.ExtensionContext): string | undefined {
  const installed = context.globalState.get<string>("server.binaryPath");
  if (installed && fs.existsSync(installed)) return installed;
  const platform = getPlatformInfo();
  const legacy = platform && path.join(context.globalStorageUri.fsPath, "bin", platform.executableName);
  return legacy && fs.existsSync(legacy) ? legacy : undefined;
}

export async function binaryVersion(binary: string): Promise<string | undefined> {
  const { stdout } = await execFileAsync(binary, ["--version"], { timeout: 5000 });
  return stdout.match(/\b(\d+\.\d+\.\d+(?:-[\w.-]+)?)/)?.[1];
}

// Releases use stable semantic versions. Never offer a downgrade or a prerelease.
export function isNewerRelease(tag: string, installed: string): boolean {
  const latest = /^v?(\d+)\.(\d+)\.(\d+)$/.exec(tag);
  const current = /^v?(\d+)\.(\d+)\.(\d+)(-.*)?$/.exec(installed);
  if (!latest || !current) return false;
  for (let i = 1; i <= 3; i++) {
    if (Number(latest[i]) !== Number(current[i])) return Number(latest[i]) > Number(current[i]);
  }
  return !!current[4];
}

export interface PlatformInfo {
  target: string;
  archiveExtension: "tar.gz" | "zip";
  executableName: string;
}

export function getPlatformInfo(): PlatformInfo | null {
  const platform = process.platform;
  const arch = process.arch;

  let archName: string | null = null;
  if (arch === "x64") {
    archName = "x86_64";
  } else if (arch === "arm64") {
    archName = "aarch64";
  } else {
    return null;
  }

  if (platform === "linux") {
    return {
      target: `${archName}-unknown-linux-musl`,
      archiveExtension: "tar.gz",
      executableName: "sanemark",
    };
  } else if (platform === "darwin") {
    return {
      target: `${archName}-apple-darwin`,
      archiveExtension: "tar.gz",
      executableName: "sanemark",
    };
  } else if (platform === "win32") {
    return {
      target: `${archName}-pc-windows-msvc`,
      archiveExtension: "zip",
      executableName: "sanemark.exe",
    };
  }

  return null;
}

export async function findInPath(executableName: string): Promise<string | null> {
  const isWindows = process.platform === "win32";
  const cmd = isWindows ? "where.exe" : "which";
  try {
    const { stdout } = await execFileAsync(cmd, [executableName]);
    const firstLine = stdout.trim().split(/\r?\n/)[0];
    if (firstLine && fs.existsSync(firstLine)) {
      return firstLine;
    }
  } catch {
    // Not found in PATH
  }
  return null;
}

export async function resolveBinary(context: vscode.ExtensionContext): Promise<string | null> {
  const config = vscode.workspace.getConfiguration("sanemark");
  const explicitPath = config.get<string>("serverPath");

  if (explicitPath && explicitPath.trim().length > 0) {
    if (fs.existsSync(explicitPath)) {
      return explicitPath;
    }
    vscode.window.showErrorMessage(
      `Sanemark: Configured serverPath does not exist: "${explicitPath}"`
    );
    return null;
  }

  const platformInfo = getPlatformInfo();
  if (!platformInfo) {
    vscode.window.showErrorMessage(
      `Sanemark: Unsupported platform or architecture (${process.platform} ${process.arch}). Please build from source and configure "sanemark.serverPath".`
    );
    return null;
  }

  // Check system PATH
  const inPath = await findInPath(platformInfo.executableName);
  if (inPath) {
    return inPath;
  }

  // Check cached global storage
  const cachedBinary = managedBinary(context);
  if (cachedBinary) {
    try {
      fs.chmodSync(cachedBinary, 0o755);
    } catch {
      // Ignore chmod errors on Windows
    }
    return cachedBinary;
  }

  // Prompt user to download
  const choice = await vscode.window.showInformationMessage(
    "Sanemark language server was not found on your system. Would you like to download the latest release binary?",
    "Download",
    "Cancel"
  );

  if (choice === "Download") {
    return await downloadLatestRelease(context, platformInfo);
  }

  return null;
}

export async function downloadLatestRelease(
  context: vscode.ExtensionContext,
  platformInfo: PlatformInfo,
  selectedRelease?: Release
): Promise<string | null> {
  return vscode.window.withProgress(
    {
      location: vscode.ProgressLocation.Notification,
      title: "Downloading Sanemark Language Server...",
      cancellable: false,
    },
    async (progress) => {
      let tempDir: string | undefined;
      try {
        progress.report({ message: "Fetching latest release information..." });
        const release = selectedRelease ?? await fetchLatestRelease();

        const expectedAssetSuffix = `${platformInfo.target}.${platformInfo.archiveExtension}`;
        const asset = release.assets.find((a) => a.name.endsWith(expectedAssetSuffix));

        if (!asset) {
          throw new Error(
            `No matching asset found for target "${platformInfo.target}" in release ${release.tag_name}.`
          );
        }

        progress.report({ message: `Downloading ${asset.name}...` });
        const assetRes = await fetch(asset.browser_download_url, { signal: AbortSignal.timeout(120_000) });
        if (!assetRes.ok) {
          throw new Error(`Failed to download binary asset: ${assetRes.statusText}`);
        }

        const arrayBuffer = await assetRes.arrayBuffer();
        const buffer = Buffer.from(arrayBuffer);

        tempDir = fs.mkdtempSync(path.join(os.tmpdir(), "sanemark-download-"));
        const archivePath = path.join(tempDir, path.basename(asset.name));
        fs.writeFileSync(archivePath, buffer);

        progress.report({ message: "Extracting archive..." });
        const extractDir = path.join(tempDir, "extracted");
        fs.mkdirSync(extractDir, { recursive: true });

        if (platformInfo.archiveExtension === "tar.gz") {
          await execFileAsync("tar", ["-xzf", archivePath, "-C", extractDir]);
        } else {
          // Windows zip
          if (process.platform === "win32") {
            try {
              await execFileAsync("tar", ["-xf", archivePath, "-C", extractDir]);
            } catch {
              await execFileAsync("powershell", [
                "-Command",
                `Expand-Archive -Path "${archivePath}" -DestinationPath "${extractDir}" -Force`,
              ]);
            }
          }
        }

        // Find executable inside extracted files
        const extractedBinary = findFileRecursive(extractDir, platformInfo.executableName);
        if (!extractedBinary) {
          throw new Error(`Executable "${platformInfo.executableName}" not found inside archive.`);
        }

        const binDir = path.join(context.globalStorageUri.fsPath, "bin");
        fs.mkdirSync(binDir, { recursive: true });
        const installDir = fs.mkdtempSync(path.join(binDir, "release-"));
        const destBinary = path.join(installDir, platformInfo.executableName);

        fs.copyFileSync(extractedBinary, destBinary);
        try {
          fs.chmodSync(destBinary, 0o755);
        } catch {
          // Windows may ignore
        }

        // Validate before selecting the new installation. Keep the old binary
        // intact for other VS Code windows that may still be using it.
        await binaryVersion(destBinary);
        await context.globalState.update("server.binaryPath", destBinary);

        vscode.window.showInformationMessage(
          `Sanemark language server (${release.tag_name}) installed successfully!`
        );
        return destBinary;
      } catch (err: any) {
        vscode.window.showErrorMessage(`Sanemark download failed: ${err?.message || err}`);
        return null;
      } finally {
        if (tempDir) {
          try { fs.rmSync(tempDir, { recursive: true, force: true }); } catch { /* Best effort */ }
        }
      }
    }
  );
}

function findFileRecursive(dir: string, targetName: string): string | null {
  const entries = fs.readdirSync(dir, { withFileTypes: true });
  for (const entry of entries) {
    const fullPath = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      const found = findFileRecursive(fullPath, targetName);
      if (found) return found;
    } else if (entry.isFile() && entry.name === targetName) {
      return fullPath;
    }
  }
  return null;
}
