# vscode-frust

A minimal, unpublished VS Code extension that registers the debug type `frust`. It only handles
launch orchestration (spawning `frust dap` and wiring up a `launch.json` entry) — stepping,
breakpoints, and variable inspection are delegated to native `lldb`-based tooling reached through
the DAP session, not reimplemented here.

## Install

This extension is not published to the Marketplace. Package and install it locally:

```bash
cd editors/vscode-frust
npx @vscode/vsce package
code --install-extension vscode-frust-<version>.vsix
```

No build step, bundler, or `node_modules` is required to package or run it — it's plain
JavaScript loaded directly by the VS Code extension host.

## Settings

| Setting | Description | Default |
|---|---|---|
| `frust.dapPath` | Path to the `frust` executable used to launch the debug adapter (`frust dap`). Leave empty to resolve `frust` from `PATH`. | `""` (resolves from PATH) |

## Example `launch.json`

```json
{
  "version": "0.2.0",
  "configurations": [
    {
      "type": "frust",
      "request": "launch",
      "name": "Launch Frust app",
      "projectRoot": "${workspaceFolder}",
      "device": "emulator-5554",
      "mode": "debug"
    }
  ]
}
```

`projectRoot` defaults to the workspace folder when omitted; `device` defaults to the desktop
preview; `mode` defaults to `debug` (`debug` | `profile` | `release`).

## Fallback without the extension

If you'd rather not install this extension, run the debug adapter as a TCP server yourself and
point `launch.json` at it directly with `debugServer` instead of `type: "frust"`:

```bash
frust dap --port 4849
```

```json
{
  "version": "0.2.0",
  "configurations": [
    {
      "request": "launch",
      "name": "Launch Frust app",
      "projectRoot": "${workspaceFolder}",
      "debugServer": 4849
    }
  ]
}
```
