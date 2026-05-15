// Based on pixel-agents by pablodelucca (https://github.com/pablodelucca/pixel-agents)
// Licensed under MIT
import { useState } from 'react';

import { useSpritePacks } from '../hooks/useSpritePacks.js';
import { isSoundEnabled, setSoundEnabled } from '../notificationSound.js';
import { vscode } from '../vscodeApi.js';
import { Button } from './ui/Button.js';
import { Checkbox } from './ui/Checkbox.js';
import { MenuItem } from './ui/MenuItem.js';
import { Modal } from './ui/Modal.js';

interface SettingsModalProps {
  isOpen: boolean;
  onClose: () => void;
  isDebugMode: boolean;
  onToggleDebugMode: () => void;
  alwaysShowOverlay: boolean;
  onToggleAlwaysShowOverlay: () => void;
  externalAssetDirectories: string[];
  watchAllSessions: boolean;
  onToggleWatchAllSessions: () => void;
  hooksEnabled: boolean;
  onToggleHooksEnabled: () => void;
  contextWindowMax: number;
  onChangeContextWindowMax: (value: number) => void;
  activeSpritePack?: string | null;
  onChangeActiveSpritePack?: (value: string | null) => void;
  notificationsEnabled: boolean;
  onToggleNotificationsEnabled: () => void;
}

const CONTEXT_WINDOW_PRESETS = [
  { label: '100k', value: 100_000 },
  { label: '200k (Sonnet 4.5)', value: 200_000 },
  { label: '500k', value: 500_000 },
  { label: '1M', value: 1_000_000 },
];
const CONTEXT_WINDOW_MIN = 10_000;
const CONTEXT_WINDOW_MAX = 2_000_000;

function clampContextWindow(v: number): number {
  if (!Number.isFinite(v)) return 200_000;
  return Math.max(CONTEXT_WINDOW_MIN, Math.min(CONTEXT_WINDOW_MAX, Math.round(v)));
}

export function SettingsModal({
  isOpen,
  onClose,
  isDebugMode,
  onToggleDebugMode,
  alwaysShowOverlay,
  onToggleAlwaysShowOverlay,
  externalAssetDirectories,
  watchAllSessions,
  onToggleWatchAllSessions,
  hooksEnabled,
  onToggleHooksEnabled,
  contextWindowMax,
  onChangeContextWindowMax,
  activeSpritePack,
  onChangeActiveSpritePack,
  notificationsEnabled,
  onToggleNotificationsEnabled,
}: SettingsModalProps) {
  const [soundLocal, setSoundLocal] = useState(isSoundEnabled);
  const { packs: spritePacks, reload: reloadSpritePacks } = useSpritePacks();
  const selectedPack = spritePacks.find((p) => p.name === activeSpritePack) ?? null;

  const isPreset = CONTEXT_WINDOW_PRESETS.some((p) => p.value === contextWindowMax);
  const [isCustom, setIsCustom] = useState(!isPreset);
  const [customInput, setCustomInput] = useState(String(contextWindowMax));

  const commitCustom = () => {
    const parsed = parseInt(customInput.replace(/[^0-9]/g, ''), 10);
    const clamped = clampContextWindow(parsed);
    setCustomInput(String(clamped));
    if (clamped !== contextWindowMax) onChangeContextWindowMax(clamped);
  };

  return (
    <Modal isOpen={isOpen} onClose={onClose} title="Settings">
      <MenuItem
        onClick={() => {
          vscode.postMessage({ type: 'openSessionsFolder' });
          onClose();
        }}
      >
        Open Sessions Folder
      </MenuItem>
      <MenuItem
        onClick={() => {
          vscode.postMessage({ type: 'exportLayout' });
          onClose();
        }}
      >
        Export Layout
      </MenuItem>
      <MenuItem
        onClick={() => {
          vscode.postMessage({ type: 'importLayout' });
          onClose();
        }}
      >
        Import Layout
      </MenuItem>
      <MenuItem
        onClick={() => {
          vscode.postMessage({ type: 'addExternalAssetDirectory' });
          onClose();
        }}
      >
        Add Asset Directory
      </MenuItem>
      {externalAssetDirectories.map((dir) => (
        <div key={dir} className="flex items-center justify-between py-4 px-10 gap-8">
          <span
            className="text-xs text-text-muted overflow-hidden text-ellipsis whitespace-nowrap"
            title={dir}
          >
            {dir.split(/[/\\]/).pop() ?? dir}
          </span>
          <Button
            variant="ghost"
            size="sm"
            onClick={() => vscode.postMessage({ type: 'removeExternalAssetDirectory', path: dir })}
            className="shrink-0"
          >
            x
          </Button>
        </div>
      ))}
      <Checkbox
        label="Sound Notifications"
        checked={soundLocal}
        onChange={() => {
          const newVal = !isSoundEnabled();
          setSoundEnabled(newVal);
          setSoundLocal(newVal);
          vscode.postMessage({ type: 'setSoundEnabled', enabled: newVal });
        }}
      />
      <Checkbox
        label="Watch All Sessions"
        checked={watchAllSessions}
        onChange={onToggleWatchAllSessions}
      />
      <Checkbox
        label="Instant Detection (Hooks)"
        checked={hooksEnabled}
        onChange={onToggleHooksEnabled}
      />
      <Checkbox
        label="OS Notifications"
        checked={notificationsEnabled}
        onChange={onToggleNotificationsEnabled}
      />
      <Checkbox
        label="Always Show Labels"
        checked={alwaysShowOverlay}
        onChange={onToggleAlwaysShowOverlay}
      />
      <div className="flex flex-col py-4 px-10 gap-2">
        <label className="text-left text-white">Context window size (tokens)</label>
        <select
          className="bg-transparent border-2 border-white/50 text-white py-2 px-2 rounded-none focus:outline-none focus:border-accent"
          value={isCustom ? 'custom' : String(contextWindowMax)}
          onChange={(e) => {
            const v = e.target.value;
            if (v === 'custom') {
              setIsCustom(true);
              setCustomInput(String(contextWindowMax));
            } else {
              setIsCustom(false);
              const num = parseInt(v, 10);
              if (Number.isFinite(num) && num !== contextWindowMax) onChangeContextWindowMax(num);
            }
          }}
        >
          {CONTEXT_WINDOW_PRESETS.map((p) => (
            <option key={p.value} value={String(p.value)} className="bg-bg text-white">
              {p.label}
            </option>
          ))}
          <option value="custom" className="bg-bg text-white">
            Custom...
          </option>
        </select>
        {isCustom && (
          <input
            type="number"
            min={CONTEXT_WINDOW_MIN}
            max={CONTEXT_WINDOW_MAX}
            step={10_000}
            value={customInput}
            onChange={(e) => setCustomInput(e.target.value)}
            onBlur={commitCustom}
            onKeyDown={(e) => {
              if (e.key === 'Enter') {
                commitCustom();
                (e.target as HTMLInputElement).blur();
              }
            }}
            className="bg-transparent border-2 border-white/50 text-white py-2 px-2 rounded-none focus:outline-none focus:border-accent"
          />
        )}
        <span className="text-xs text-text-muted">
          Affects token health bar thresholds. Default: 200 000 (Sonnet 4.5).
        </span>
      </div>
      <div className="flex flex-col py-4 px-10 gap-2">
        <div className="flex items-center justify-between">
          <label className="text-left text-white">Sprite Pack</label>
          <Button
            variant="ghost"
            size="sm"
            onClick={reloadSpritePacks}
            title="Re-scan ~/.pixel-agents/sprites/"
          >
            ↻
          </Button>
        </div>
        <select
          className="bg-transparent border-2 border-white/50 text-white py-2 px-2 rounded-none focus:outline-none focus:border-accent"
          value={activeSpritePack ?? ''}
          onChange={(e) => {
            const v = e.target.value;
            const next = v === '' ? null : v;
            if (onChangeActiveSpritePack) onChangeActiveSpritePack(next);
            vscode.postMessage({
              type: 'updateSettings',
              settings: { activeSpritePack: next },
            });
            // Pack changes require a full reload of decoded assets.
            // eslint-disable-next-line @typescript-eslint/no-explicit-any
            const refresh = (window as unknown as { __pixelAgentsRefresh?: () => void })
              .__pixelAgentsRefresh;
            if (typeof refresh === 'function') refresh();
          }}
        >
          <option value="" className="bg-bg text-white">
            Default (built-in)
          </option>
          {spritePacks.map((p) => (
            <option key={p.name} value={p.name} className="bg-bg text-white">
              {p.name} (v{p.version})
            </option>
          ))}
        </select>
        {selectedPack && (selectedPack.description || selectedPack.author) && (
          <span className="text-xs text-text-muted italic">
            {selectedPack.description ?? ''}
            {selectedPack.description && selectedPack.author ? ' — ' : ''}
            {selectedPack.author ? `by ${selectedPack.author}` : ''}
          </span>
        )}
        {spritePacks.length === 0 && (
          <span className="text-xs text-text-muted">
            Drop a pack folder in <code>~/.pixel-agents/sprites/</code> then refresh.
          </span>
        )}
      </div>
      <Checkbox label="Debug View" checked={isDebugMode} onChange={onToggleDebugMode} />
    </Modal>
  );
}
