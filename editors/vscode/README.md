# Novo for Visual Studio Code

Syntax highlighting for `.nv` files, plus the Novo language server: type errors
as you type, the type of whatever is under the cursor on hover, Go to
Definition, and Format Document.

The language server is `novo lsp`, so `novo` must be on your `PATH`, or set
`novo.path` to the executable.

## Installing from source

```bash
cd editors/vscode
npm install
npx @vscode/vsce package
code --install-extension novo-0.1.0.vsix
```
