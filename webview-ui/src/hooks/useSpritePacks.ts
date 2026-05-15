/**
 * useSpritePacks — discovers custom sprite packs in ~/.pixel-agents/sprites/.
 *
 * Calls the `list_sprite_packs` Tauri command at mount and exposes the result.
 * Falls back to an empty list when running outside Tauri (no-op in dev/browser).
 */
import { useCallback, useEffect, useState } from 'react';

export interface SpritePackInfo {
  name: string;
  version: string;
  author?: string | null;
  description?: string | null;
  path: string;
  characterCount: number;
}

function isTauriEnv(): boolean {
  return (
    typeof window !== 'undefined' &&
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    typeof (window as any).__TAURI_INTERNALS__ !== 'undefined'
  );
}

export function useSpritePacks(): {
  packs: SpritePackInfo[];
  reload: () => void;
  loading: boolean;
} {
  const [packs, setPacks] = useState<SpritePackInfo[]>([]);
  const [loading, setLoading] = useState(false);

  const reload = useCallback(() => {
    if (!isTauriEnv()) {
      setPacks([]);
      return;
    }
    setLoading(true);
    void import('@tauri-apps/api/core')
      .then(({ invoke }) => invoke<SpritePackInfo[]>('list_sprite_packs'))
      .then((result) => setPacks(Array.isArray(result) ? result : []))
      .catch((e) => {
        console.warn('[useSpritePacks] list_sprite_packs failed:', e);
        setPacks([]);
      })
      .finally(() => setLoading(false));
  }, []);

  useEffect(() => {
    reload();
  }, [reload]);

  return { packs, reload, loading };
}
