import { afterEach, expect, mock, test } from "bun:test";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

mock.module("vscode", () => ({}));
const { isNewerRelease, managedBinary, getPlatformInfo } = await import("../src/downloader");

const directories: string[] = [];
afterEach(() => {
  for (const dir of directories.splice(0)) fs.rmSync(dir, { recursive: true, force: true });
});

test("offers newer stable releases without downgrading or repeatedly offering the same version", () => {
  expect(isNewerRelease("v0.10.0", "0.9.9")).toBe(true);
  expect(isNewerRelease("v1.0.0", "0.99.0")).toBe(true);
  expect(isNewerRelease("v1.2.4", "1.2.3")).toBe(true);
  expect(isNewerRelease("v1.2.3", "1.2.3")).toBe(false);
  expect(isNewerRelease("v1.2.2", "1.2.3")).toBe(false);
  expect(isNewerRelease("v1.2.3-beta.1", "1.2.2")).toBe(false);
  expect(isNewerRelease("v1.2.3", "1.2.3-beta.1")).toBe(true);
  expect(isNewerRelease("invalid", "1.2.3")).toBe(false);
});

test("selects the new installation and falls back to the legacy cache if missing", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "sanemark-test-"));
  directories.push(root);
  const legacy = path.join(root, "bin", getPlatformInfo()!.executableName);
  fs.mkdirSync(path.dirname(legacy));
  fs.writeFileSync(legacy, "legacy");
  const installed = path.join(root, "new-server");
  const context = {
    globalStorageUri: { fsPath: root },
    globalState: { get: () => installed },
  } as any;
  expect(managedBinary(context)).toBe(legacy);
  fs.writeFileSync(installed, "new");
  expect(managedBinary(context)).toBe(installed);
  fs.unlinkSync(installed);
  expect(managedBinary(context)).toBe(legacy);
  fs.unlinkSync(legacy);
  expect(managedBinary(context)).toBeUndefined();
});
