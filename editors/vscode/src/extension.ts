import * as vscode from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  TransportKind,
} from "vscode-languageclient/node";

let client: LanguageClient | undefined;

/** Resolve the `luabox` binary from the `luabox.path` setting, falling back to PATH. */
function resolveBinary(): string {
  const configured = vscode.workspace
    .getConfiguration("luabox")
    .get<string>("path");
  const path = (configured ?? "").trim();
  // A bare name (default "luabox") is left as-is so the OS resolves it on PATH.
  return path.length > 0 ? path : "luabox";
}

function buildClient(): LanguageClient {
  const command = resolveBinary();
  // `luabox lsp` speaks LSP over stdio (the server has no separate --stdio flag).
  const serverOptions: ServerOptions = {
    run: { command, args: ["lsp"], transport: TransportKind.stdio },
    debug: { command, args: ["lsp"], transport: TransportKind.stdio },
  };

  const clientOptions: LanguageClientOptions = {
    // Attach to plain Lua files. We deliberately do NOT declare the `lua`
    // language (avoids clobbering other Lua extensions' grammars) and only
    // select documents by language id here.
    //
    // This selector also scopes the capabilities the server advertises:
    // vscode-languageclient negotiates formatting, range formatting and
    // semantic tokens automatically (no extra client code), registering the
    // providers for exactly these documents. The server's semantic-token
    // legend uses only standard token types/modifiers, so themes work
    // without `semanticTokenScopes` contributions.
    documentSelector: [{ scheme: "file", language: "lua" }],
    synchronize: {
      fileEvents: vscode.workspace.createFileSystemWatcher("**/*.{lua,toml}"),
    },
  };

  return new LanguageClient(
    "luabox",
    "luabox Language Server",
    serverOptions,
    clientOptions
  );
}

async function startClient(): Promise<void> {
  client = buildClient();
  await client.start();
}

async function stopClient(): Promise<void> {
  if (client) {
    await client.stop();
    client = undefined;
  }
}

export function activate(context: vscode.ExtensionContext): void {
  context.subscriptions.push(
    vscode.commands.registerCommand("luabox.restartServer", async () => {
      await stopClient();
      await startClient();
      vscode.window.showInformationMessage("luabox language server restarted.");
    })
  );

  void startClient().catch((err) => {
    vscode.window.showErrorMessage(
      `luabox: failed to start language server (${resolveBinary()} lsp): ${err}`
    );
  });
}

export async function deactivate(): Promise<void> {
  await stopClient();
}
