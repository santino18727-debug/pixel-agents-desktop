/**
 * Tauri shim - bridges the VS Code extension message protocol to Tauri IPC.
 *
 * Additions vs original:
 *  - P1: load_layout command invoked at bootstrap (persisted layout takes priority over default)
 *  - P1: saveLayout postMessage case added to handleOutboundMessage -> invoke save_layout
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

async function bootstrap(): Promise<void> {
  try {
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
      ? import("@tauri-apps/api/core").then(({ invoke }) =>
          invoke<SessionRecord[]>("list_sessions", { maxAgeHours: 24, limit: 20 }).catch(() => []),
        )
      : Promise.resolve([]);

    // P1: Load persisted layout from Rust backend (returns null if absent or corrupt).
    const persistedLayoutPromise: Promise<unknown | null> = isTauri
      ? import("@tauri-apps/api/core").then(({ invoke }) =>
          invoke<unknown | null>("load_layout").catch(() => null),
        )
      : Promise.resolve(null);

    const [characters, floors, walls, furniture, furnitureCatalog, defaultLayout, sessions, persistedLayout] =
      await Promise.all([
        fetchJson<unknown>("/assets/decoded/characters.json"),
        fetchJson<unknown>("/assets/decoded/floors.json"),
        fetchJson<unknown>("/assets/decoded/walls.json"),
        fetchJson<unknown>("/assets/decoded/furniture.json"),
        fetchJson<unknown>("/assets/furniture-catalog.json").catch(() => []),
        fetchJson<unknown>("/assets/default-layout-1.json").catch(() => null),
        sessionPromise,
        persistedLayoutPromise,
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
    }

    // P1: Use persisted layout if available, otherwise fall back to default layout.
    const layoutToLoad = persistedLayout ?? defaultLayout;
    dispatch({ type: "layoutLoaded", layout: layoutToLoad, wasReset: false });

    dispatch({
      type: "settingsLoaded",
      soundEnabled: false,
      watchAllSessions: true,
      alwaysOnTop: false,
      extensionVersion: "0.1.0",
      lastSeenVersion: "",
      externalAssetDirectories: [],
    });

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

async function subscribeTauriEvents(): Promise<void> {
  try {
    const { listen } = await import("@tauri-apps/api/event");
    await listen<unknown>("agent-event", (event) => dispatch(event.payload));
    await listen<unknown>("settings-changed", (event) =>
      dispatch({ type: "settingsLoaded", ...Object(event.payload) }),
    );
    await listen<unknown>("update-agents", (event) => dispatch(event.payload));
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
      case "saveLayout":
        // P1: persist layout to ~/.pixel-agents/layout.json via Rust
        await invoke("save_layout", { layout: m.layout }).catch((e: unknown) => {
          console.error("[Tauri shim] save_layout failed:", e);
        });
        break;
      default:
        console.debug("[Tauri shim] Unhandled postMessage:", m.type);
    }
  } catch (e) {
    console.error("[Tauri shim] handleOutboundMessage failed:", e);
  }
}
