import * as vscode from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  Executable,
  ExecuteCommandRequest,
} from "vscode-languageclient/node";
import {
  resolveBinary,
  downloadLatestRelease,
  getPlatformInfo,
} from "./downloader";

let client: LanguageClient | undefined;
let log: vscode.LogOutputChannel | undefined;

export async function activate(context: vscode.ExtensionContext) {
  log = vscode.window.createOutputChannel("Sanemark", { log: true });
  context.subscriptions.push(log);

  await startServer(context);

  // Register commands
  context.subscriptions.push(
    vscode.commands.registerCommand("sanemark.moveReferencesToBottom", async () => {
      await executeDocumentCommand("sanemark.moveReferencesToBottom");
    }),

    vscode.commands.registerCommand("sanemark.inlineReferences", async () => {
      await executeDocumentCommand("sanemark.inlineReferences");
    }),

    vscode.commands.registerCommand("sanemark.copyAsInlined", async () => {
      await executeRangeCommand("sanemark.copyAsInlined");
    }),

    vscode.commands.registerCommand("sanemark.convertToInline", async () => {
      await executeRangeCommand("sanemark.convertToInline");
    }),

    vscode.commands.registerCommand("sanemark.openDailyNote", async () => {
      await executeServerCommand("sanemark.openDailyNote", [0]);
    }),

    vscode.commands.registerCommand("sanemark.openYesterdayNote", async () => {
      await executeServerCommand("sanemark.openDailyNote", [-1]);
    }),

    vscode.commands.registerCommand("sanemark.openTomorrowNote", async () => {
      await executeServerCommand("sanemark.openDailyNote", [1]);
    }),

    vscode.commands.registerCommand("sanemark.restartServer", async () => {
      await restartServer(context);
    }),

    vscode.commands.registerCommand("sanemark.downloadServer", async () => {
      const platform = getPlatformInfo();
      if (!platform) {
        vscode.window.showErrorMessage(
          `Sanemark: Unsupported platform (${process.platform} ${process.arch}).`
        );
        return;
      }
      const newBinary = await downloadLatestRelease(context, platform);
      if (newBinary) {
        await restartServer(context);
      }
    })
  );

  // Restart on relevant configuration changes
  context.subscriptions.push(
    vscode.workspace.onDidChangeConfiguration(async (event) => {
      if (event.affectsConfiguration("sanemark.serverPath")) {
        const choice = await vscode.window.showInformationMessage(
          "Sanemark serverPath changed. Restart server now?",
          "Restart",
          "Later"
        );
        if (choice === "Restart") {
          await restartServer(context);
        }
      }
    })
  );
}

export async function deactivate(): Promise<void> {
  if (client) {
    await client.stop();
    client = undefined;
  }
}

async function startServer(context: vscode.ExtensionContext): Promise<void> {
  const binaryPath = await resolveBinary(context);
  if (!binaryPath) {
    log?.warn("No Sanemark language server executable was selected.");
    return;
  }

  log?.info(`Using language server executable: ${binaryPath}`);

  const serverExecutable: Executable = {
    command: binaryPath,
    args: ["lsp"],
  };

  const serverOptions: ServerOptions = {
    run: serverExecutable,
    debug: serverExecutable,
  };

  const clientOptions: LanguageClientOptions = {
    documentSelector: [
      { scheme: "file", language: "markdown" },
      { scheme: "untitled", language: "markdown" },
    ],
    synchronize: {
      configurationSection: "sanemark",
    },
    initializationOptions: getClientConfig(),
  };

  client = new LanguageClient(
    "sanemark",
    "Sanemark Language Server",
    serverOptions,
    clientOptions
  );

  try {
    await client.start();
    log?.info("Language server started.");
  } catch (err: any) {
    log?.error(`Failed to start language server: ${err?.message || err}`);
    vscode.window.showErrorMessage(
      `Failed to start Sanemark language server: ${err?.message || err}`
    );
  }
}

async function restartServer(context: vscode.ExtensionContext): Promise<void> {
  if (client) {
    try {
      await client.stop();
    } catch {
      // Ignore stop errors during restart
    }
    client = undefined;
  }
  await startServer(context);
  vscode.window.showInformationMessage("Sanemark language server restarted.");
}

function getClientConfig(): Record<string, any> {
  const config = vscode.workspace.getConfiguration("sanemark");
  return {
    sanemark: {
      gfm: config.get<boolean>("gfm", true),
      formatting: config.get<object>("formatting"),
      diagnostics: config.get<object>("diagnostics"),
      completion: config.get<object>("completion"),
      snippets: config.get<object>("snippets"),
      journal: config.get<object>("journal"),
    },
  };
}

async function executeDocumentCommand(command: string): Promise<void> {
  const editor = vscode.window.activeTextEditor;
  if (!editor || editor.document.languageId !== "markdown") {
    vscode.window.showWarningMessage("Open a Markdown document to run this command.");
    return;
  }

  if (!client || !client.isRunning()) {
    vscode.window.showWarningMessage("Sanemark language server is not running.");
    return;
  }

  try {
    await client.sendRequest(ExecuteCommandRequest.type, {
      command,
      arguments: [editor.document.uri.toString()],
    });
  } catch (err: any) {
    vscode.window.showErrorMessage(`Sanemark command failed: ${err?.message || err}`);
  }
}

async function executeRangeCommand(command: string): Promise<void> {
  const editor = vscode.window.activeTextEditor;
  if (!editor || editor.document.languageId !== "markdown") {
    vscode.window.showWarningMessage("Open a Markdown document to run this command.");
    return;
  }

  if (!client || !client.isRunning()) {
    vscode.window.showWarningMessage("Sanemark language server is not running.");
    return;
  }

  const range = editor.selection.isEmpty
    ? undefined
    : client.code2ProtocolConverter.asRange(editor.selection);

  try {
    await client.sendRequest(ExecuteCommandRequest.type, {
      command,
      arguments: [editor.document.uri.toString(), range],
    });
  } catch (err: any) {
    vscode.window.showErrorMessage(`Sanemark command failed: ${err?.message || err}`);
  }
}

async function executeServerCommand(command: string, args: any[]): Promise<void> {
  if (!client || !client.isRunning()) {
    vscode.window.showWarningMessage("Sanemark language server is not running.");
    return;
  }

  try {
    await client.sendRequest(ExecuteCommandRequest.type, {
      command,
      arguments: args,
    });
  } catch (err: any) {
    vscode.window.showErrorMessage(`Sanemark command failed: ${err?.message || err}`);
  }
}
