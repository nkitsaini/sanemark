import { expect, test } from "bun:test";
import { readFileSync } from "fs";

test("editor commands do not collide with server-advertised commands", () => {
  const server = readFileSync(new URL("../../../src/server.rs", import.meta.url), "utf8");
  const serverCommands = [...server.matchAll(/const CMD_\w+: &str = "([^"]+)"/g)].map(match => match[1]);
  expect(serverCommands.length).toBeGreaterThan(0);
  const extension = readFileSync(new URL("../src/extension.ts", import.meta.url), "utf8");
  const registered = [...extension.matchAll(/registerCommand\("([^"]+)"/g)].map(match => match[1]);
  const manifest = JSON.parse(readFileSync(new URL("../package.json", import.meta.url), "utf8"));
  for (const command of manifest.contributes.commands) {
    expect(registered).toContain(command.command);
    expect(serverCommands).not.toContain(command.command);
    expect(command.title.startsWith("Sanemark:")).toBe(false);
  }
  expect(extension.indexOf('registerCommand("sanemark.checkForUpdates"')).toBeLessThan(extension.indexOf("await startServer(context)"));
});
