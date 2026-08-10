'use strict';

const vscode = require('vscode');

/**
 * Resolves the `frust` executable to invoke for the debug adapter.
 *
 * Honors the `frust.dapPath` user/workspace setting when set; otherwise falls back to the
 * literal `frust`, resolved from PATH by the OS at spawn time.
 *
 * @returns {string}
 */
function resolveDapPath() {
  const configured = vscode.workspace.getConfiguration('frust').get('dapPath');
  if (typeof configured === 'string' && configured.trim().length > 0) {
    return configured;
  }
  return 'frust';
}

/**
 * @implements {vscode.DebugAdapterDescriptorFactory}
 */
class FrustDebugAdapterDescriptorFactory {
  // eslint-disable-next-line no-unused-vars
  createDebugAdapterDescriptor(session, executable) {
    return new vscode.DebugAdapterExecutable(resolveDapPath(), ['dap']);
  }
}

/**
 * @implements {vscode.DebugConfigurationProvider}
 */
class FrustDebugConfigurationProvider {
  resolveDebugConfiguration(folder, config) {
    if (!config.projectRoot) {
      if (folder && folder.uri) {
        config.projectRoot = folder.uri.fsPath;
      } else {
        vscode.window.showErrorMessage(
          'Frust: no "projectRoot" set and no workspace folder is open — set "projectRoot" ' +
            'explicitly in launch.json or open a folder.'
        );
        return undefined;
      }
    }
    return config;
  }
}

/**
 * @param {vscode.ExtensionContext} context
 */
function activate(context) {
  context.subscriptions.push(
    vscode.debug.registerDebugAdapterDescriptorFactory('frust', new FrustDebugAdapterDescriptorFactory())
  );
  context.subscriptions.push(
    vscode.debug.registerDebugConfigurationProvider('frust', new FrustDebugConfigurationProvider())
  );
}

function deactivate() {}

module.exports = {
  activate,
  deactivate,
};
