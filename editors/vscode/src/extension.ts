import * as vscode from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  State,
  TransportKind,
} from "vscode-languageclient/node";

let client: LanguageClient | undefined;
let statusItem: vscode.StatusBarItem | undefined;
/** Subscription to the current client's onDidChangeState; recreated per client. */
let stateSubscription: vscode.Disposable | undefined;

/** Resolve the `luabox` binary from the `luabox.path` setting, falling back to PATH. */
function resolveBinary(): string {
  const configured = vscode.workspace
    .getConfiguration("luabox")
    .get<string>("path");
  const path = (configured ?? "").trim();
  // A bare name (default "luabox") is left as-is so the OS resolves it on PATH.
  return path.length > 0 ? path : "luabox";
}

/** Render the status bar item for the given language-server state. */
function updateStatus(state: State): void {
  if (!statusItem) {
    return;
  }
  switch (state) {
    case State.Starting:
      statusItem.text = "$(sync~spin) luabox";
      statusItem.tooltip = "luabox language server: starting…";
      statusItem.backgroundColor = undefined;
      break;
    case State.Running:
      statusItem.text = "$(check) luabox";
      statusItem.tooltip =
        `luabox language server: running\n` +
        `Binary: ${resolveBinary()}\n` +
        `Click to restart`;
      statusItem.backgroundColor = undefined;
      break;
    case State.Stopped:
    default:
      statusItem.text = "$(error) luabox";
      statusItem.tooltip =
        "luabox language server: stopped — click to restart";
      statusItem.backgroundColor = new vscode.ThemeColor(
        "statusBarItem.errorBackground"
      );
      break;
  }
}

/** Show the status item only while a Lua document is the active editor. */
function updateVisibility(): void {
  if (!statusItem) {
    return;
  }
  if (vscode.window.activeTextEditor?.document.languageId === "lua") {
    statusItem.show();
  } else {
    statusItem.hide();
  }
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
  // Re-attach the state listener on every restart (the client is recreated).
  stateSubscription = client.onDidChangeState((e) => updateStatus(e.newState));
  // Reflect the imminent spawn immediately; onDidChangeState follows.
  updateStatus(State.Starting);
  await client.start();
}

async function stopClient(): Promise<void> {
  stateSubscription?.dispose();
  stateSubscription = undefined;
  if (client) {
    await client.stop();
    client = undefined;
  }
  updateStatus(State.Stopped);
}

export function activate(context: vscode.ExtensionContext): void {
  statusItem = vscode.window.createStatusBarItem(
    "luabox.serverStatus",
    vscode.StatusBarAlignment.Left
  );
  statusItem.name = "luabox Language Server";
  statusItem.command = "luabox.restartServer";
  context.subscriptions.push(statusItem);

  updateVisibility();
  context.subscriptions.push(
    vscode.window.onDidChangeActiveTextEditor(updateVisibility)
  );

  context.subscriptions.push(
    vscode.commands.registerCommand("luabox.restartServer", async () => {
      await stopClient();
      await startClient();
      vscode.window.showInformationMessage("luabox language server restarted.");
    })
  );

  void startClient().catch((err) => {
    // onDidChangeState may not fire on spawn failure (e.g. binary not found),
    // so land the error rendering explicitly.
    updateStatus(State.Stopped);
    vscode.window.showErrorMessage(
      `luabox: failed to start language server (${resolveBinary()} lsp): ${err}`
    );
  });
}

export async function deactivate(): Promise<void> {
  stateSubscription?.dispose();
  stateSubscription = undefined;
  await stopClient();
  statusItem?.dispose();
  statusItem = undefined;
}
