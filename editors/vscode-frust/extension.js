'use strict';

const vscode = require('vscode');

/**
 * @implements {vscode.DebugAdapterDescriptorFactory}
 */
class FrustDebugAdapterDescriptorFactory {
  // eslint-disable-next-line no-unused-vars
  createDebugAdapterDescriptor(session, executable) {
    // Resolve port from launch config's debugServer, workspace setting, or default
    let port = 4849; // default
    if (session.configuration && typeof session.configuration.debugServer === 'number') {
      port = session.configuration.debugServer;
    } else {
      const configured = vscode.workspace.getConfiguration('frust').get('dapPort');
      if (typeof configured === 'number') {
        port = configured;
      }
    }
    return new vscode.DebugAdapterServer(port);
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
