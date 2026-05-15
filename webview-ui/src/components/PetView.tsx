// Pet Mode view: a tiny, transparent webview that displays the single most
// recently active agent. Renders in a 200x200 always-on-top window.
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';

import { getCharacterSprites, setCharacterTemplates } from '../office/sprites/spriteData.js';
import { getCachedSprite } from '../office/sprites/spriteCache.js';
import { Direction } from '../office/types.js';

// Tauri APIs are dynamically imported so this component still works in a
// browser dev runtime that has no Tauri bridge.
async function invokeTauri<T = unknown>(cmd: string, args?: Record<string, unknown>): Promise<T | undefined> {
  try {
    const { invoke } = await import('@tauri-apps/api/core');
    return (await invoke(cmd, args)) as T;
  } catch (e) {
    console.warn('[PetView] invoke failed', cmd, e);
    return undefined;
  }
}

const PET_SIZE = 200;
const SPRITE_ZOOM = 5; // Character sprites are ~16x32, so 5x ≈ 80x160 in window

interface AgentState {
  id: number;
  palette: number;
  hueShift: number;
  status: string;
  lastSeen: number;
}

export function PetView() {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const agentsRef = useRef<Map<number, AgentState>>(new Map());
  const [activeAgentId, setActiveAgentId] = useState<number | null>(null);
  const [frame, setFrame] = useState(0);
  const [spritesReady, setSpritesReady] = useState(false);

  // Force-pick agent id from URL if provided (?agentId=N).
  const urlAgentId = useMemo(() => {
    const v = new URLSearchParams(window.location.search).get('agentId');
    if (!v) return null;
    const n = Number(v);
    return Number.isFinite(n) ? n : null;
  }, []);

  // Load character sprite templates from the static `decoded/` bundle the main
  // window also relies on. We do this independently so the pet window does not
  // need the full extension-message bootstrap.
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const res = await fetch('/assets/decoded/characters.json');
        if (res.ok) {
          // eslint-disable-next-line @typescript-eslint/no-explicit-any
          const palettes = (await res.json()) as any;
          if (!cancelled && Array.isArray(palettes) && palettes.length > 0) {
            setCharacterTemplates(palettes);
          }
        }
        if (!cancelled) setSpritesReady(true);
      } catch (e) {
        console.warn('[PetView] sprite load failed', e);
        if (!cancelled) setSpritesReady(true);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  // Subscribe to Tauri agent-event stream and maintain the active agent.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    (async () => {
      try {
        const { listen } = await import('@tauri-apps/api/event');
        unlisten = await listen<unknown>('agent-event', (event) => {
          const payload = event.payload as { type?: string; id?: number; status?: string; palette?: number; hueShift?: number };
          if (!payload || typeof payload.id !== 'number') return;
          const id = payload.id;
          const existing = agentsRef.current.get(id) ?? {
            id,
            palette: 0,
            hueShift: 0,
            status: 'idle',
            lastSeen: 0,
          };
          if (typeof payload.palette === 'number') existing.palette = payload.palette;
          if (typeof payload.hueShift === 'number') existing.hueShift = payload.hueShift;
          if (payload.type === 'agentStatus' && payload.status) {
            existing.status = payload.status;
            if (payload.status === 'active' || payload.status === 'permission') {
              existing.lastSeen = Date.now();
            }
          }
          if (payload.type === 'agentClosed') {
            agentsRef.current.delete(id);
            setActiveAgentId((cur) => (cur === id ? null : cur));
            return;
          }
          agentsRef.current.set(id, existing);

          // Auto-select most-recently-active agent (unless URL pin is set).
          if (urlAgentId === null) {
            const ordered = [...agentsRef.current.values()].sort((a, b) => b.lastSeen - a.lastSeen);
            const best = ordered.find((a) => a.lastSeen > 0) ?? ordered[0];
            if (best && best.id !== activeAgentId) setActiveAgentId(best.id);
          }
        });
      } catch (e) {
        console.warn('[PetView] listen failed', e);
      }
    })();
    return () => {
      if (unlisten) unlisten();
    };
  }, [urlAgentId, activeAgentId]);

  // If URL pinned, set the active id immediately.
  useEffect(() => {
    if (urlAgentId !== null) setActiveAgentId(urlAgentId);
  }, [urlAgentId]);

  // Animation tick — alternate frame every 300ms (typing animation).
  useEffect(() => {
    const t = window.setInterval(() => setFrame((f) => (f + 1) % 2), 300);
    return () => window.clearInterval(t);
  }, []);

  // Esc closes the pet window (toggles off).
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        void invokeTauri('toggle_pet_mode');
      }
    };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, []);

  // Draw the active character sprite, centered.
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;
    ctx.imageSmoothingEnabled = false;
    ctx.clearRect(0, 0, PET_SIZE, PET_SIZE);

    if (!spritesReady) return;
    const agent = activeAgentId !== null ? agentsRef.current.get(activeAgentId) : null;
    const palette = agent?.palette ?? 0;
    const hueShift = agent?.hueShift ?? 0;
    try {
      const sprites = getCharacterSprites(palette, hueShift);
      // Use typing animation facing down — looks like an agent at work.
      const pair = sprites.typing[Direction.DOWN];
      const spriteData = pair[frame] ?? pair[0];
      if (!spriteData || spriteData.length === 0) return;
      const cached = getCachedSprite(spriteData, SPRITE_ZOOM);
      const dx = Math.round((PET_SIZE - cached.width) / 2);
      const dy = Math.round((PET_SIZE - cached.height) / 2);
      ctx.drawImage(cached, dx, dy);
    } catch (e) {
      console.warn('[PetView] render failed', e);
    }
  }, [frame, activeAgentId, spritesReady]);

  const handleClick = useCallback(() => {
    void invokeTauri('focus_main_window');
  }, []);

  return (
    <div
      data-tauri-drag-region
      style={{
        width: PET_SIZE,
        height: PET_SIZE,
        background: 'transparent',
        position: 'relative',
        overflow: 'hidden',
        userSelect: 'none',
      }}
      title="Drag to move • Click to focus main • Esc to close"
    >
      <canvas
        ref={canvasRef}
        width={PET_SIZE}
        height={PET_SIZE}
        onClick={handleClick}
        style={{
          position: 'absolute',
          inset: 0,
          cursor: 'pointer',
          imageRendering: 'pixelated',
          background: 'transparent',
        }}
      />
    </div>
  );
}

export default PetView;
