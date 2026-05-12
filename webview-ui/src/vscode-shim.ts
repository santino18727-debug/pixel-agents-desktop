/**
 * Tauri shim — bridges the VS Code extension message protocol to Tauri IPC.
 *
 * Imported as the very first line of main.tsx when running inside Tauri.
 * Must execute before React bootstraps so the message handler is ready.
 *
 * Strategy:
 *  - Detect Tauri by checking window.__TAURI_INTERNALS__
 *  - Override vscodeApi.ts's acquireVsCodeApi path via monkey-patching
 *    the global window object so runtime.ts sees it as 'vscode' runtime
 *  - Translate outbound postMessage calls into Tauri invoke() calls
 *  - Subscribe to Tauri events and re-dispatch them as MessageEvent on window
 */

// Only activate inside Tauri webview
const isTauri =
  typeof window !== 'undefined' &&
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  typeof (window as any).__TAURI_INTERNALS__ !== 'undefined';

if (isTauri) {
  installTauriShim();
}

function installTauriShim(): void {
  // ── 1. Pretend acquireVsCodeApi exists so runtime.ts detects 'vscode' runtime ──
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (window as any).acquireVsCodeApi = () => ({
    postMessage: handleOutboundMessage,
  });

  // ── 2. Subscribe to Tauri events and forward them as window MessageEvents ──
  void subscribeToTauriEvents();
}

/**
 * Translate postMessage calls from the webview into Tauri invoke() calls.
 */
async function handleOutboundMessage(msg: unknown): Promise<void> {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const { invoke } = await import('@tauri-apps/api/core');
  const m = msg as { type?: string; [key: string]: unknown };

  switch (m.type) {
    case 'webviewReady':
      // No-op — Tauri doesn't need an explicit ready signal
      break;

    case 'getSettings':
      try {
        const settings = await invoke('get_settings');
        dispatchToWindow({ type: 'settingsLoaded', ...Object(settings) });
      } catch (e) {
        console.error('[Tauri shim] get_settings failed:', e);
      }
      break;

    case 'updateSettings':
      try {
        await invoke('set_settings', { settings: m.settings });
      } catch (e) {
        console.error('[Tauri shim] set_settings failed:', e);
      }
      break;

    case 'watchSession':
      // Phase 2 — no-op in Phase 0/1
      break;

    case 'watchAll':
      try {
        await invoke('set_settings', {
          settings: { watchAll: true, theme: 'dark', alwaysOnTop: false },
        });
      } catch (e) {
        console.error('[Tauri shim] set_watch_all failed:', e);
      }
      break;

    default:
      // Forward unknown messages as-is for future extensibility
      console.debug('[Tauri shim] Unhandled postMessage:', m.type);
  }
}

/**
 * Subscribe to Tauri events that replace the VS Code extension push messages.
 */
async function subscribeToTauriEvents(): Promise<void> {
  try {
    const { listen } = await import('@tauri-apps/api/event');

    // File watcher parsed events → re-dispatch as window messages
    await listen<unknown>('agent-event', (event) => {
      dispatchToWindow(event.payload);
    });

    // Settings changed externally (e.g. from another window)
    await listen<unknown>('settings-changed', (event) => {
      dispatchToWindow({ type: 'settingsLoaded', ...Object(event.payload) });
    });

    // Session list refresh
    await listen<unknown>('update-agents', (event) => {
      dispatchToWindow(event.payload);
    });

    // On ready: fetch initial settings and sessions
    await bootstrapInitialState();
  } catch (e) {
    console.error('[Tauri shim] Failed to subscribe to Tauri events:', e);
  }
}

/**
 * Fetch initial state from Rust backend and dispatch to the webview.
 * Called once after event listeners are registered.
 */
async function bootstrapInitialState(): Promise<void> {
  try {
    const { invoke } = await import('@tauri-apps/api/core');

    const settings = await invoke<{
      watchAll: boolean;
      theme: string;
      alwaysOnTop: boolean;
    }>('get_settings').catch(() => ({
      watchAll: false,
      theme: 'dark',
      alwaysOnTop: false,
    }));

    dispatchToWindow({
      type: 'settingsLoaded',
      soundEnabled: false,
      watchAllSessions: settings.watchAll,
      alwaysOnTop: settings.alwaysOnTop,
      extensionVersion: '0.1.0',
      lastSeenVersion: '',
      externalAssetDirectories: [],
    });

    const sessions = await invoke<
      Array<{
        session_id: string;
        project_dir: string;
        is_subagent: boolean;
        jsonl_path: string;
      }>
    >('list_sessions').catch(() => []);

    if (sessions.length > 0) {
      dispatchToWindow({
        type: 'existingAgents',
        agents: sessions
          .filter((s) => !s.is_subagent)
          .map((_, i) => i + 1),
        agentMeta: {},
        folderNames: {},
      });
    }
  } catch (e) {
    console.error('[Tauri shim] Bootstrap failed:', e);
  }
}

function dispatchToWindow(data: unknown): void {
  window.dispatchEvent(new MessageEvent('message', { data }));
}
