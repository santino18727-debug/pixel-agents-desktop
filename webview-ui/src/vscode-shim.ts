/**
 * Tauri shim — bridges the VS Code extension message protocol to Tauri IPC.
 *
 * Strategy:
 *  1. Always install acquireVsCodeApi so runtime.ts sees 'vscode' runtime
 *  2. Fetch decoded assets from Vite dev server (same endpoints as browserMock)
 *  3. Fetch sessions from Rust backend in parallel with assets (Tauri only)
 *  4. Dispatch sprite → existingAgents → layoutLoaded in correct order so agents
 *     are buffered in pendingAgents before layout builds seats
 *  5. Subscribe to Tauri events for live session updates
 */

function dispatch(data: unknown): void {
  window.dispatchEvent(new MessageEvent('message', { data }));
}

// ── 1. Always install acquireVsCodeApi ─────────────────────────────────────
// eslint-disable-next-line @typescript-eslint/no-explicit-any
(window as any).acquireVsCodeApi = () => ({
  postMessage: handleOutboundMessage,
});

// Detect Tauri context (for live events only — assets always fetched from Vite)
const isTauri =
  typeof window !== 'undefined' &&
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  typeof (window as any).__TAURI_INTERNALS__ !== 'undefined';

// Bootstrap AFTER React mounts and useExtensionMessages registers its listener.
// Double-RAF ensures two render frames have passed (mount + effects).
let bootstrapScheduled = false;
function scheduleBootstrap(): void {
  if (bootstrapScheduled) return;
  bootstrapScheduled = true;
  requestAnimationFrame(() => requestAnimationFrame(() => void bootstrap()));
}

window.addEventListener('DOMContentLoaded', scheduleBootstrap);
if (document.readyState !== 'loading') {
  scheduleBootstrap();
}

// ── 2. Bootstrap: load assets + sessions, then unlock UI ──────────────────
async function bootstrap(): Promise<void> {
  try {
    // Fetch sessions from Rust backend in parallel with sprite assets so we can
    // dispatch existingAgents BEFORE layoutLoaded (agents get buffered into
    // pendingAgents and receive correct seat assignment when layout arrives).
    type SessionRecord = {
      session_id: string;
      project_dir: string;
      /** Human-readable project name, decoded from Claude Code path encoding. */
      folder_name: string;
      is_subagent: boolean;
      jsonl_path: string;
      modified_secs: number;
    };

    const sessionPromise: Promise<SessionRecord[]> = isTauri
      ? import('@tauri-apps/api/core').then(({ invoke }) =>
          invoke<SessionRecord[]>('list_sessions', { maxAgeHours: 4, limit: 10 }).catch(() => []),
        )
      : Promise.resolve([]);

    // Load decoded sprite assets from Vite middleware endpoints.
    // Order: characterSpritesLoaded → floorTilesLoaded →
    //        wallTilesLoaded → furnitureAssetsLoaded → existingAgents → layoutLoaded
    const [characters, floors, walls, furniture, furnitureCatalog, defaultLayout, sessions] =
      await Promise.all([
        fetchJson<unknown>('/assets/decoded/characters.json'),
        fetchJson<unknown>('/assets/decoded/floors.json'),
        fetchJson<unknown>('/assets/decoded/walls.json'),
        fetchJson<unknown>('/assets/decoded/furniture.json'),
        fetchJson<unknown>('/assets/furniture-catalog.json').catch(() => []),
        fetchJson<unknown>('/assets/default-layout-1.json').catch(() => null),
        sessionPromise,
      ]);

    dispatch({ type: 'characterSpritesLoaded', characters });
    dispatch({ type: 'floorTilesLoaded', sprites: floors });
    dispatch({ type: 'wallTilesLoaded', sets: walls });

    // furnitureAssetsLoaded expects { catalog: CatalogEntry[], sprites: Record<string, string[][]> }
    dispatch({
      type: 'furnitureAssetsLoaded',
      catalog: furnitureCatalog,
      sprites: furniture,
    });

    // Dispatch existingAgents BEFORE layoutLoaded so the handler buffers them
    // into pendingAgents — they are flushed with correct seat assignment when
    // layoutLoaded fires below.
    const mainSessions = sessions.filter((s) => !s.is_subagent);
    if (mainSessions.length > 0) {
      const agentIds = mainSessions.map((_, i) => i + 1);
      const folderNames: Record<number, string> = {};
      mainSessions.forEach((s, i) => {
        // folder_name is decoded by the Rust backend (Claude Code path encoding -> readable name)
        folderNames[i + 1] = s.folder_name || s.project_dir.split(/[\/]/).pop() || s.project_dir;
      });
      dispatch({
        type: 'existingAgents',
        agents: agentIds,
        agentMeta: {},
        folderNames,
      });
    }

    // layoutLoaded with default layout JSON → renders furniture + flushes pending agents
    dispatch({ type: 'layoutLoaded', layout: defaultLayout, wasReset: false });

    dispatch({
      type: 'settingsLoaded',
      soundEnabled: false,
      watchAllSessions: true,
      alwaysOnTop: false,
      extensionVersion: '0.1.0',
      lastSeenVersion: '',
      externalAssetDirectories: [],
    });

    // Subscribe to live Tauri events (new sessions, JSONL updates)
    if (isTauri) {
      await subscribeTauriEvents();
    }
  } catch (e) {
    console.error('[Tauri shim] Bootstrap failed:', e);
    // Fallback: unlock UI with empty layout so user sees something
    dispatch({ type: 'layoutLoaded', layout: null, wasReset: false });
  }
}

async function fetchJson<T>(url: string): Promise<T> {
  const res = await fetch(url);
  if (!res.ok) throw new Error(`HTTP ${res.status} for ${url}`);
  return res.json() as Promise<T>;
}

// ── 3. Subscribe to Tauri events for live updates ─────────────────────────
async function subscribeTauriEvents(): Promise<void> {
  try {
    const { listen } = await import('@tauri-apps/api/event');
    await listen<unknown>('agent-event', (event) => dispatch(event.payload));
    await listen<unknown>('settings-changed', (event) =>
      dispatch({ type: 'settingsLoaded', ...Object(event.payload) }),
    );
    await listen<unknown>('update-agents', (event) => dispatch(event.payload));
  } catch (e) {
    console.error('[Tauri shim] subscribeTauriEvents failed:', e);
  }
}

// ── 4. Handle outbound postMessage from webview ───────────────────────────
async function handleOutboundMessage(msg: unknown): Promise<void> {
  const m = msg as { type?: string; [key: string]: unknown };

  if (!isTauri) {
    console.debug('[Tauri shim] postMessage (no Tauri):', m.type);
    return;
  }

  try {
    const { invoke } = await import('@tauri-apps/api/core');
    switch (m.type) {
      case 'getSettings': {
        const settings = await invoke('get_settings').catch(() => ({}));
        dispatch({ type: 'settingsLoaded', ...Object(settings) });
        break;
      }
      case 'updateSettings':
        await invoke('set_settings', { settings: m.settings }).catch(() => {});
        break;
      default:
        console.debug('[Tauri shim] Unhandled postMessage:', m.type);
    }
  } catch (e) {
    console.error('[Tauri shim] handleOutboundMessage failed:', e);
  }
}
