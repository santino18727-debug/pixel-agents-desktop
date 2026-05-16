export {};

declare global {
  interface Window {
    __pixelAgentsRefresh?: () => void;
    acquireVsCodeApi?: () => { postMessage: (msg: unknown) => void };
    __TAURI_INTERNALS__?: unknown;
  }
}
