/**
 * Tauri shim - bridges the VS Code extension message protocol to Tauri IPC.
 *
 * Additions vs original:
 *  - P1: load_layout command invoked at bootstrap (persisted layout takes priority over default)
 *  - P1: saveLayout postMessage case added to handleOutboundMessage -> invoke save_layout
 *  - P2: workspaceFolders dispatched after existingAgents so frontend shows folder names
 *  - P3: get_settings invoked in parallel at bootstrap; values merged with hardcoded defaults
 */

function dispatch(data: unknown): void {
  window.dispatchEvent(new MessageEvent("message", { data }));
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
(window as any).acquireVsCodeApi = () => ({
  postMessage: handleOutboundMessage,
});

const isTauri =
  typeof window !== "undefined" &&
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  typeof (window as any).__TAURI_INTERNALS__ !== "undefined";

let bootstrapScheduled = false;
function scheduleBootstrap(): void {
  if (bootstrapScheduled) return;
  bootstrapScheduled = true;
  requestAnimationFrame(() => requestAnimationFrame(() => void bootstrap()));
}

window.addEventListener("DOMContentLoaded", scheduleBootstrap);
if (document.readyState !== "loading") {
  scheduleBootstrap();
}

// Exposed so the UI refresh button can re-run the full bootstrap sequence.
// eslint-disable-next-line @typescript-eslint/no-explicit-any
(window as any).__pixelAgentsRefresh = () => void bootstrap();

async function bootstrap(): Promise<void> {
  try {
    type SessionRecord = {
      session_id: string;
      project_dir: string;
      /** Human-readable project name, decoded from Claude Code path encoding. */
      folder_name: string;
      is_subagent: boolean;
      /** For sub-agent sessions, the session_id of the parent (derived from path). */
      parent_session_id: string | null;
      jsonl_path: string;
      modified_secs: number;
    };

    type PersistedSettings = {
      soundEnabled?: boolean;
      watchAllSessions?: boolean;
      alwaysShowLabels?: boolean;
      alwaysOnTop?: boolean;
      extensionVersion?: string;
      lastSeenVersion?: string;
      externalAssetDirectories?: string[];
      maxSessions?: number;
      hooksEnabled?: boolean;
    };

    // Load settings first to get maxSessions, then fetch sessions.
    // We start both promises in parallel; settings loads fast from tauri-plugin-store.
    const settingsEarlyPromise: Promise<PersistedSettings | null> = isTauri
      ? import("@tauri-apps/api/core").then(({ invoke }) =>
          invoke<PersistedSettings>("get_settings").catch(() => null),
        )
      : Promise.resolve(null);

    const sessionPromise: Promise<SessionRecord[]> = settingsEarlyPromise.then((s) => {
      const limit = Math.min(Math.max(s?.maxSessions ?? 8, 1), 50);
      return isTauri
        ? import("@tauri-apps/api/core").then(({ invoke }) =>
            invoke<SessionRecord[]>("list_sessions", { maxAgeHours: 2, limit }).catch(() => []),
          )
        : Promise.resolve([]);
    });

    // P1: Load persisted layout from Rust backend (returns null if absent or corrupt).
    const persistedLayoutPromise: Promise<unknown | null> = isTauri
      ? import("@tauri-apps/api/core").then(({ invoke }) =>
          invoke<unknown | null>("load_layout").catch(() => null),
        )
      : Promise.resolve(null);

    const [characters, floors, walls, furniture, furnitureCatalog, defaultLayout, sessions, persistedLayout, persistedSettings] =
      await Promise.all([
        fetchJson<unknown>("/assets/decoded/characters.json"),
        fetchJson<unknown>("/assets/decoded/floors.json"),
        fetchJson<unknown>("/assets/decoded/walls.json"),
        fetchJson<unknown>("/assets/decoded/furniture.json"),
        fetchJson<unknown>("/assets/furniture-catalog.json").catch(() => []),
        fetchJson<unknown>("/assets/default-layout-2.json").catch(() => null),
        sessionPromise,
        persistedLayoutPromise,
        settingsEarlyPromise,
      ]);

    dispatch({ type: "characterSpritesLoaded", characters });
    dispatch({ type: "floorTilesLoaded", sprites: floors });
    dispatch({ type: "wallTilesLoaded", sets: walls });

    dispatch({
      type: "furnitureAssetsLoaded",
      catalog: furnitureCatalog,
      sprites: furniture,
    });

    const mainSessions = sessions.filter((s) => !s.is_subagent);
    if (mainSessions.length > 0) {
      const agentIds = mainSessions.map((_, i) => i + 1);
      const folderNames: Record<number, string> = {};
      mainSessions.forEach((s, i) => {
        folderNames[i + 1] = s.folder_name || s.project_dir.split(/[\/]/).pop() || s.project_dir;
      });
      dispatch({
        type: "existingAgents",
        agents: agentIds,
        agentMeta: {},
        folderNames,
      });

      // Always start existing sessions as idle at boot — the file watcher will quickly
      // re-activate characters if Claude Code is actively running tools.
      mainSessions.forEach((_s, i) => {
        dispatch({ type: "agentStatus", id: i + 1, status: "idle" });
      });
      // P2: dispatch workspaceFolders so the frontend can display folder names per agent.
      dispatch({
        type: "workspaceFolders",
        folders: mainSessions.map((s) => ({
          name: s.folder_name || s.project_dir.split(/[\/]/).pop() || s.project_dir,
          path: s.project_dir,
        })),
      });

      // Dispatch existing sub-agents as agentCreated with isTeammate: true.
      // IDs continue the sequence after mainSessions (1-based).
      const subSessions = sessions.filter((s) => s.is_subagent);
      subSessions.forEach((sub, subIdx) => {
        const parentSession = mainSessions.find(
          (m) => m.session_id === sub.parent_session_id,
        );
        if (!parentSession) return; // parent not found — skip silently
        const parentId = mainSessions.indexOf(parentSession) + 1;
        const subId = mainSessions.length + subIdx + 1;
        dispatch({
          type: "agentCreated",
          id: subId,
          isTeammate: true,
          parentAgentId: parentId,
          folderName: sub.folder_name || sub.project_dir.split(/[\/]/).pop() || sub.project_dir,
        });
      });
    } else {
      // No active sessions found — inform frontend to show an empty-state overlay.
      dispatch({ type: "noAgents" });
    }

    // P1: Use persisted layout if available, otherwise fall back to default layout.
    const layoutToLoad = persistedLayout ?? defaultLayout;
    dispatch({ type: "layoutLoaded", layout: layoutToLoad, wasReset: false });

    // P3: Hardcoded defaults, overridden by any values returned from get_settings.
    const settingsDefaults: PersistedSettings = {
      soundEnabled: false,
      watchAllSessions: true,
      alwaysOnTop: false,
      extensionVersion: "0.1.0",
      lastSeenVersion: "",
      externalAssetDirectories: [],
      maxSessions: 8,
    };
    const mergedSettings = persistedSettings
      ? { ...settingsDefaults, ...persistedSettings }
      : settingsDefaults;

    dispatch({
      type: "settingsLoaded",
      ...mergedSettings,
    });

    // F4: Load external assets if configured.
    if (
      isTauri &&
      mergedSettings.externalAssetDirectories &&
      mergedSettings.externalAssetDirectories.length > 0
    ) {
      try {
        const { invoke } = await import("@tauri-apps/api/core");
        const externalAssets = await invoke<{
          catalog: unknown[];
          sprites: unknown;
          characters: unknown[];
        }>("scan_external_assets", {
          dirs: mergedSettings.externalAssetDirectories,
        }).catch(() => null);
        if (externalAssets) {
          dispatch({
            type: "furnitureAssetsLoaded",
            catalog: externalAssets.catalog,
            sprites: externalAssets.sprites,
          });
        }
      } catch (e) {
        console.warn("[Tauri shim] scan_external_assets failed:", e);
      }
    }

    if (isTauri) {
      await subscribeTauriEvents();
    }
  } catch (e) {
    console.error("[Tauri shim] Bootstrap failed:", e);
    dispatch({ type: "layoutLoaded", layout: null, wasReset: false });
  }
}

async function fetchJson<T>(url: string): Promise<T> {
  const res = await fetch(url);
  if (!res.ok) throw new Error("HTTP " + res.status + " for " + url);
  return res.json() as Promise<T>;
}

// Module-level unlisten store
let _unlistenFns: Array<() => void> = [];

async function subscribeTauriEvents(): Promise<void> {
  // Clean up previous listeners first
  for (const fn of _unlistenFns) fn();
  _unlistenFns = [];
  try {
    const { listen } = await import("@tauri-apps/api/event");
    _unlistenFns.push(await listen<unknown>("agent-event", (event) => dispatch(event.payload)));
    _unlistenFns.push(await listen<unknown>("settings-changed", (event) =>
      dispatch({ type: "settingsLoaded", ...Object(event.payload) }),
    ));
    _unlistenFns.push(await listen<unknown>("update-agents", (event) => dispatch(event.payload)));
    // F2: Re-dispatch layoutLoaded when layout.json is edited externally.
    _unlistenFns.push(await listen<unknown>("layout-changed", (event) =>
      dispatch({ type: "layoutLoaded", layout: event.payload, wasReset: false }),
    ));
  } catch (e) {
    console.error("[Tauri shim] subscribeTauriEvents failed:", e);
  }
}

async function handleOutboundMessage(msg: unknown): Promise<void> {
  const m = msg as { type?: string; [key: string]: unknown };

  if (!isTauri) {
    console.debug("[Tauri shim] postMessage (no Tauri):", m.type);
    return;
  }

  try {
    const { invoke } = await import("@tauri-apps/api/core");
    switch (m.type) {
      case "getSettings": {
        const settings = await invoke("get_settings").catch(() => ({}));
        dispatch({ type: "settingsLoaded", ...Object(settings) });
        break;
      }
      case "updateSettings":
        await invoke("set_settings", { settings: m.settings }).catch(() => {});
        break;
      case "setSoundEnabled":
        await invoke("set_settings", { settings: { soundEnabled: m.enabled } }).catch(() => {});
        break;
      case "setWatchAllSessions":
        await invoke("set_settings", { settings: { watchAllSessions: m.enabled } }).catch(() => {});
        break;
      case "setAlwaysShowLabels":
        await invoke("set_settings", { settings: { alwaysShowLabels: m.enabled } }).catch(() => {});
        break;
      case "setHooksEnabled":
        await invoke("set_settings", { settings: { hooksEnabled: m.enabled } }).catch(() => {});
        break;
      case "saveLayout":
        // P1: persist layout to ~/.pixel-agents/layout.json via Rust
        await invoke("save_layout", { layout: m.layout }).catch((e: unknown) => {
          console.error("[Tauri shim] save_layout failed:", e);
        });
        break;
      case "exportLayout": {
        // Trigger download via a data URL — no Rust needed
        const json = JSON.stringify(m.layout, null, 2);
        const blob = new Blob([json], { type: "application/json" });
        const url = URL.createObjectURL(blob);
        const a = document.createElement("a");
        a.href = url;
        a.download = "pixel-agents-layout.json";
        a.click();
        URL.revokeObjectURL(url);
        break;
      }
      case "importLayout": {
        // Open file picker
        const input = document.createElement("input");
        input.type = "file";
        input.accept = ".json";
        input.onchange = async () => {
          const file = input.files?.[0];
          if (!file) return;
          try {
            const text = await file.text();
            const layout = JSON.parse(text);
            dispatch({ type: "layoutLoaded", layout, wasReset: false });
          } catch (e) {
            console.error("[Tauri shim] importLayout parse error:", e);
          }
        };
        input.click();
        break;
      }
      case "addExternalAssetDirectory":
      case "removeExternalAssetDirectory": {
        // Update settings with new dirs list
        await invoke("set_settings", { settings: { externalAssetDirectories: m.directories } }).catch(() => {});
        break;
      }
      case "openSessionsFolder": {
        await invoke("open_sessions_folder").catch(() => {});
        break;
      }
      case "saveAgentSeats":
      case "webviewReady":
        // No-op in Tauri mode
        break;
      default:
        console.debug("[Tauri shim] Unhandled postMessage:", m.type);
    }
  } catch (e) {
    console.error("[Tauri shim] handleOutboundMessage failed:", e);
  }
}
