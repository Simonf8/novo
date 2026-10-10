const vscode = require("vscode");
const { LanguageClient } = require("vscode-languageclient/node");

let client;

function activate(context) {
  const command = vscode.workspace.getConfiguration("novo").get("path") || "novo";
  client = new LanguageClient(
    "novo",
    "Novo",
    { command, args: ["lsp"] },
    { documentSelector: [{ scheme: "file", language: "novo" }] }
  );
  client.start().catch((err) => {
    vscode.window.showErrorMessage(
      `Could not start \`${command} lsp\`: ${err.message}. Set "novo.path" to the novo executable.`
    );
  });
  context.subscriptions.push({ dispose: () => client && client.stop() });
}

function deactivate() {
  return client ? client.stop() : undefined;
}

module.exports = { activate, deactivate };
