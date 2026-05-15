// Based on pixel-agents by pablodelucca (https://github.com/pablodelucca/pixel-agents)
// Licensed under MIT
import { useEffect, useRef, useState } from 'react';

import { playDoneSound, playPermissionSound, setSoundEnabled } from '../notificationSound.js';
import { setContextWindowMax, type OfficeState } from '../office/engine/officeState.js';
import { setFloorSprites } from '../office/floorTiles.js';
import { buildDynamicCatalog } from '../office/layout/furnitureCatalog.js';
import { migrateLayoutColors } from '../office/layout/layoutSerializer.js';
import { setCharacterTemplates } from '../office/sprites/spriteData.js';
import { enrichToolStatus, extractToolName } from '../office/toolUtils.js';
import type { OfficeLayout, ToolActivity } from '../office/types.js';
import { setWallSprites } from '../office/wallTiles.js';
import { vscode } from '../vscodeApi.js';

export interface SubagentCharacter {
  id: number;
  parentAgentId: number;
  parentToolId: string;
  label: string;
}

interface FurnitureAsset {
  id: string;
  name: string;
  label: string;
  category: string;
  file: string;
  width: number;
  height: number;
  footprintW: number;
  footprintH: number;
  isDesk: boolean;
  canPlaceOnWalls: boolean;
  groupId?: string;
  canPlaceOnSurfaces?: boolean;
  backgroundTiles?: number;
  orientation?: string;
  state?: string;
  mirrorSide?: boolean;
  rotationScheme?: string;
  animationGroup?: string;
  frame?: number;
}

export interface WorkspaceFolder {
  name: string;
  path: string;
}

interface ExtensionMessageState {
  agents: number[];
  selectedAgent: number | null;
  agentTools: Record<number, ToolActivity[]>;
  agentStatuses: Record<number, string>;
  subagentTools: Record<number, Record<string, ToolActivity[]>>;
  subagentCharacters: SubagentCharacter[];
  layoutReady: boolean;
  layoutWasReset: boolean;
  loadedAssets?: { catalog: FurnitureAsset[]; sprites: Record<string, string[][]> };
  workspaceFolders: WorkspaceFolder[];
  externalAssetDirectories: string[];
  lastSeenVersion: string;
  extensionVersion: string;
  watchAllSessions: boolean;
  setWatchAllSessions: (v: boolean) => void;
  alwaysShowLabels: boolean;
  hooksEnabled: boolean;
  setHooksEnabled: (v: boolean) => void;
  notificationsEnabled: boolean;
  setNotificationsEnabled: (v: boolean) => void;
  hooksInfoShown: boolean;
  noAgents: boolean;
  contextWindowMax: number;
  setContextWindowMaxState: (v: number) => void;
}

function saveAgentSeats(os: OfficeState): void {
  const seats: Record<number, { palette: number; hueShift: number; seatId: string | null }> = {};
  for (const ch of os.characters.values()) {
    if (ch.isSubagent) continue;
    seats[ch.id] = { palette: ch.palette, hueShift: ch.hueShift, seatId: ch.seatId };
  }
  vscode.postMessage({ type: 'saveAgentSeats', seats });
}

export function useExtensionMessages(
  getOfficeState: () => OfficeState,
  onLayoutLoaded?: (layout: OfficeLayout) => void,
  isEditDirty?: () => boolean,
): ExtensionMessageState {
  const [agents, setAgents] = useState<number[]>([]);
  const [selectedAgent, setSelectedAgent] = useState<number | null>(null);
  const [agentTools, setAgentTools] = useState<Record<number, ToolActivity[]>>({});
  const [agentStatuses, setAgentStatuses] = useState<Record<number, string>>({});
  const [subagentTools, setSubagentTools] = useState<
    Record<number, Record<string, ToolActivity[]>>
  >({});
  const [subagentCharacters, setSubagentCharacters] = useState<SubagentCharacter[]>([]);
  const [layoutReady, setLayoutReady] = useState(false);
  const [layoutWasReset, setLayoutWasReset] = useState(false);
  const [loadedAssets, setLoadedAssets] = useState<
    { catalog: FurnitureAsset[]; sprites: Record<string, string[][]> } | undefined
  >();
  const [workspaceFolders, setWorkspaceFolders] = useState<WorkspaceFolder[]>([]);
  const [externalAssetDirectories, setExternalAssetDirectories] = useState<string[]>([]);
  const [lastSeenVersion, setLastSeenVersion] = useState('');
  const [extensionVersion, setExtensionVersion] = useState('');
  const [watchAllSessions, setWatchAllSessions] = useState(false);
  const [alwaysShowLabels, setAlwaysShowLabels] = useState(false);
  const [hooksEnabled, setHooksEnabled] = useState(true);
  const [notificationsEnabled, setNotificationsEnabled] = useState(true);
  const [hooksInfoShown, setHooksInfoShown] = useState(true);
  const [noAgents, setNoAgents] = useState(false);
  const [contextWindowMax, setContextWindowMaxState] = useState(200_000);

  // Track whether initial layout has been loaded (ref to avoid re-render)
  const layoutReadyRef = useRef(false);

  useEffect(() => {
    // Buffer agents from existingAgents until layout is loaded
    let pendingAgents: Array<{
      id: number;
      palette?: number;
      hueShift?: number;
      seatId?: string;
      folderName?: string;
    }> = [];

    // ── Dispatch table ───────────────────────────────────────────────────────
    // Each entry is a named handler function that closes over local state and
    // setters. The main handler below does a single O(1) lookup instead of a
    // chain of 20+ else-if branches.
    // ────────────────────────────────────────────────────────────────────────

    type Msg = Record<string, unknown>;

    const handleLayoutLoaded = (msg: Msg, os: OfficeState) => {
      if (layoutReadyRef.current && isEditDirty?.()) {
        return;
      }
      const rawLayout = msg.layout as OfficeLayout | null;
      const layout = rawLayout && rawLayout.version === 1 ? migrateLayoutColors(rawLayout) : null;
      if (layout) {
        os.rebuildFromLayout(layout);
        onLayoutLoaded?.(layout);
      } else {
        onLayoutLoaded?.(os.getLayout());
      }
      for (const p of pendingAgents) {
        os.addAgent(p.id, p.palette, p.hueShift, p.seatId, true, p.folderName);
      }
      pendingAgents = [];
      layoutReadyRef.current = true;
      setLayoutReady(true);
      if (msg.wasReset) setLayoutWasReset(true);
      if (os.characters.size > 0) saveAgentSeats(os);
    };

    const handleNoAgents = () => setNoAgents(true);

    const handleAgentCreated = (msg: Msg, os: OfficeState) => {
      setNoAgents(false);
      const id = msg.id as number;
      const folderName = msg.folderName as string | undefined;
      const isTeammate = msg.isTeammate as boolean | undefined;
      const teammateName = msg.teammateName as string | undefined;
      const teammateParentId = msg.parentAgentId as number | undefined;
      const teamName = msg.teamName as string | undefined;
      setAgents((prev) => (prev.includes(id) ? prev : [...prev, id]));
      if (!isTeammate) setSelectedAgent(id);
      if (isTeammate && teammateParentId !== undefined) {
        const parentCh = os.characters.get(teammateParentId);
        os.addAgent(id, parentCh?.palette, parentCh?.hueShift, undefined, undefined, parentCh?.folderName);
        const ch = os.characters.get(id);
        if (ch) {
          ch.leadAgentId = teammateParentId;
          ch.teamName = teamName ?? parentCh?.teamName;
          ch.agentName = teammateName;
        }
      } else {
        os.addAgent(id, undefined, undefined, undefined, undefined, folderName);
      }
      saveAgentSeats(os);
    };

    const handleAgentClosed = (msg: Msg, os: OfficeState) => {
      const id = msg.id as number;
      setAgents((prev) => prev.filter((a) => a !== id));
      setSelectedAgent((prev) => (prev === id ? null : prev));
      setAgentTools((prev) => { const n = { ...prev }; delete n[id]; return n; });
      setAgentStatuses((prev) => { const n = { ...prev }; delete n[id]; return n; });
      setSubagentTools((prev) => { const n = { ...prev }; delete n[id]; return n; });
      os.removeAllSubagents(id);
      setSubagentCharacters((prev) => prev.filter((s) => s.parentAgentId !== id));
      os.removeAgent(id);
    };

    const handleExistingAgents = (msg: Msg) => {
      const incoming = msg.agents as number[];
      const meta = (msg.agentMeta || {}) as Record<number, { palette?: number; hueShift?: number; seatId?: string }>;
      const folderNames = (msg.folderNames || {}) as Record<number, string>;
      for (const id of incoming) {
        const m = meta[id];
        pendingAgents.push({ id, palette: m?.palette, hueShift: m?.hueShift, seatId: m?.seatId, folderName: folderNames[id] });
      }
      setAgents((prev) => {
        const ids = new Set(prev);
        const merged = [...prev];
        for (const id of incoming) { if (!ids.has(id)) merged.push(id); }
        return merged.sort((a, b) => a - b);
      });
    };

    const handleAgentSelected = (msg: Msg) => setSelectedAgent(msg.id as number);

    const handleAgentStatus = (msg: Msg, os: OfficeState) => {
      const id = msg.id as number;
      const status = msg.status as string;
      setAgentStatuses((prev) => {
        if (status === 'active') {
          if (!(id in prev)) return prev;
          const n = { ...prev }; delete n[id]; return n;
        }
        return { ...prev, [id]: status };
      });
      os.setAgentActive(id, status === 'active');
      if (status === 'waiting') { os.showWaitingBubble(id); playDoneSound(); }
    };

    const handleAgentToolStart = (msg: Msg, os: OfficeState) => {
      const id = msg.id as number;
      const toolId = msg.toolId as string;
      const toolName = (msg.toolName as string | undefined) ?? '';
      const toolInput = (msg.toolInput as Record<string, unknown> | undefined) ?? undefined;
      const rawStatus = msg.status as string;
      const permissionActive = msg.permissionActive as boolean | undefined;
      const status = enrichToolStatus(toolName || rawStatus, toolInput);
      setAgentTools((prev) => {
        const list = prev[id] || [];
        if (list.some((t) => t.toolId === toolId)) return prev;
        return { ...prev, [id]: [...list, { toolId, status, done: false, permissionWait: permissionActive || false }] };
      });
      const effectiveToolName = toolName || extractToolName(rawStatus) || '';
      os.setAgentTool(id, effectiveToolName);
      os.setAgentActive(id, true);
      if (!permissionActive) os.clearPermissionBubble(id);
      const runInBackground = msg.runInBackground as boolean | undefined;
      if ((effectiveToolName === 'Task' || effectiveToolName === 'Agent') && !runInBackground && !toolId.startsWith('hook-')) {
        const label = rawStatus.startsWith('Subtask:') ? rawStatus.slice('Subtask:'.length).trim() : '';
        const subId = os.addSubagent(id, toolId);
        setSubagentCharacters((prev) => {
          if (prev.some((s) => s.id === subId)) return prev;
          return [...prev, { id: subId, parentAgentId: id, parentToolId: toolId, label }];
        });
      }
    };

    const handleAgentToolDone = (msg: Msg) => {
      const id = msg.id as number;
      const toolId = msg.toolId as string;
      setAgentTools((prev) => {
        const list = prev[id];
        if (!list) return prev;
        return { ...prev, [id]: list.map((t) => (t.toolId === toolId ? { ...t, done: true } : t)) };
      });
    };

    const handleAgentToolsClear = (msg: Msg, os: OfficeState) => {
      const id = msg.id as number;
      setAgentTools((prev) => { const n = { ...prev }; delete n[id]; return n; });
      setSubagentTools((prev) => { const n = { ...prev }; delete n[id]; return n; });
      const clearCh = os.characters.get(id);
      const hasInlineTeammates = clearCh?.teamName && clearCh?.isTeamLead && !clearCh?.teamUsesTmux;
      if (!hasInlineTeammates) {
        os.removeAllSubagents(id);
        setSubagentCharacters((prev) => prev.filter((s) => s.parentAgentId !== id));
      }
      os.setAgentTool(id, null);
      os.clearPermissionBubble(id);
    };

    const handleAgentToolPermission = (msg: Msg, os: OfficeState) => {
      const id = msg.id as number;
      setAgentTools((prev) => {
        const list = prev[id];
        if (!list) return prev;
        return { ...prev, [id]: list.map((t) => (t.done ? t : { ...t, permissionWait: true })) };
      });
      os.showPermissionBubble(id);
      playPermissionSound();
    };

    const handleAgentToolPermissionClear = (msg: Msg, os: OfficeState) => {
      const id = msg.id as number;
      setAgentTools((prev) => {
        const list = prev[id];
        if (!list) return prev;
        if (!list.some((t) => t.permissionWait)) return prev;
        return { ...prev, [id]: list.map((t) => (t.permissionWait ? { ...t, permissionWait: false } : t)) };
      });
      os.clearPermissionBubble(id);
      for (const [subId, meta] of os.subagentMeta) {
        if (meta.parentAgentId === id) os.clearPermissionBubble(subId);
      }
    };

    const handleSubagentToolPermission = (msg: Msg, os: OfficeState) => {
      const subId = os.getSubagentId(msg.id as number, msg.parentToolId as string);
      if (subId !== null) os.showPermissionBubble(subId);
    };

    const handleSubagentToolStart = (msg: Msg, os: OfficeState) => {
      const id = msg.id as number;
      const parentToolId = msg.parentToolId as string;
      const toolId = msg.toolId as string;
      const subToolName = (msg.toolName as string | undefined) ?? '';
      const subToolInput = (msg.toolInput as Record<string, unknown> | undefined) ?? undefined;
      const rawSubStatus = msg.status as string;
      const status = enrichToolStatus(subToolName || rawSubStatus, subToolInput);
      setSubagentTools((prev) => {
        const agentSubs = prev[id] || {};
        const list = agentSubs[parentToolId] || [];
        if (list.some((t) => t.toolId === toolId)) return prev;
        return { ...prev, [id]: { ...agentSubs, [parentToolId]: [...list, { toolId, status, done: false }] } };
      });
      const subId = os.getSubagentId(id, parentToolId);
      if (subId !== null) { os.setAgentTool(subId, subToolName || extractToolName(rawSubStatus)); os.setAgentActive(subId, true); }
    };

    const handleSubagentToolDone = (msg: Msg) => {
      const id = msg.id as number;
      const parentToolId = msg.parentToolId as string;
      const toolId = msg.toolId as string;
      setSubagentTools((prev) => {
        const agentSubs = prev[id];
        if (!agentSubs) return prev;
        const list = agentSubs[parentToolId];
        if (!list) return prev;
        return { ...prev, [id]: { ...agentSubs, [parentToolId]: list.map((t) => (t.toolId === toolId ? { ...t, done: true } : t)) } };
      });
    };

    const handleSubagentClear = (msg: Msg, os: OfficeState) => {
      const id = msg.id as number;
      const parentToolId = msg.parentToolId as string;
      setSubagentTools((prev) => {
        const agentSubs = prev[id];
        if (!agentSubs || !(parentToolId in agentSubs)) return prev;
        const next = { ...agentSubs };
        delete next[parentToolId];
        if (Object.keys(next).length === 0) { const outer = { ...prev }; delete outer[id]; return outer; }
        return { ...prev, [id]: next };
      });
      os.removeSubagent(id, parentToolId);
      setSubagentCharacters((prev) => prev.filter((s) => !(s.parentAgentId === id && s.parentToolId === parentToolId)));
    };

    const handleCharacterSpritesLoaded = (msg: Msg) => {
      const chars = msg.characters as Array<{ down: string[][][]; up: string[][][]; right: string[][][] }>;
      setCharacterTemplates(chars);
    };

    const handleFloorTilesLoaded = (msg: Msg) => {
      const sprites = msg.sprites as string[][][];
      setFloorSprites(sprites);
    };

    const handleWallTilesLoaded = (msg: Msg) => {
      const sets = msg.sets as string[][][][];
      setWallSprites(sets);
    };

    const handleFurnitureAssetsLoaded = (msg: Msg) => {
      try {
        const catalog = msg.catalog as FurnitureAsset[];
        const sprites = msg.sprites as Record<string, string[][]>;
        buildDynamicCatalog({ catalog, sprites });
        setLoadedAssets({ catalog, sprites });
      } catch (err) {
        console.error('[Webview] Error processing furnitureAssetsLoaded:', err);
      }
    };

    const handleWorkspaceFolders = (msg: Msg) => setWorkspaceFolders(msg.folders as WorkspaceFolder[]);

    const handleSettingsLoaded = (msg: Msg) => {
      setSoundEnabled(msg.soundEnabled as boolean);
      if (typeof msg.watchAllSessions === 'boolean') setWatchAllSessions(msg.watchAllSessions as boolean);
      if (typeof msg.alwaysShowLabels === 'boolean') setAlwaysShowLabels(msg.alwaysShowLabels as boolean);
      if (typeof msg.hooksEnabled === 'boolean') setHooksEnabled(msg.hooksEnabled as boolean);
      if (typeof msg.notificationsEnabled === 'boolean') setNotificationsEnabled(msg.notificationsEnabled as boolean);
      if (typeof msg.hooksInfoShown === 'boolean') setHooksInfoShown(msg.hooksInfoShown as boolean);
      if (Array.isArray(msg.externalAssetDirectories)) setExternalAssetDirectories(msg.externalAssetDirectories as string[]);
      if (typeof msg.lastSeenVersion === 'string') setLastSeenVersion(msg.lastSeenVersion as string);
      if (typeof msg.extensionVersion === 'string') setExtensionVersion(msg.extensionVersion as string);
      if (typeof msg.defaultContextWindowMax === 'number') {
        setContextWindowMax(msg.defaultContextWindowMax as number);
        setContextWindowMaxState(msg.defaultContextWindowMax as number);
      }
    };

    const handleExternalAssetDirectoriesUpdated = (msg: Msg) => {
      if (Array.isArray(msg.dirs)) setExternalAssetDirectories(msg.dirs as string[]);
    };

    const handleAgentTeamInfo = (msg: Msg, os: OfficeState) => {
      os.setTeamInfo(
        msg.id as number,
        msg.teamName as string | undefined,
        msg.agentName as string | undefined,
        msg.isTeamLead as boolean | undefined,
        msg.leadAgentId as number | undefined,
        msg.teamUsesTmux as boolean | undefined,
      );
    };

    const handleAgentTokenUsage = (msg: Msg, os: OfficeState) =>
      os.setAgentTokens(msg.id as number, msg.inputTokens as number, msg.outputTokens as number);

    // Dispatch table — O(1) lookup replacing 20+ else-if branches.
    const handlers: Record<string, (msg: Msg, os: OfficeState) => void> = {
      layoutLoaded: handleLayoutLoaded,
      noAgents: handleNoAgents,
      agentCreated: handleAgentCreated,
      agentClosed: handleAgentClosed,
      existingAgents: handleExistingAgents,
      agentSelected: handleAgentSelected,
      agentStatus: handleAgentStatus,
      agentToolStart: handleAgentToolStart,
      agentToolDone: handleAgentToolDone,
      agentToolsClear: handleAgentToolsClear,
      agentToolPermission: handleAgentToolPermission,
      agentToolPermissionClear: handleAgentToolPermissionClear,
      subagentToolPermission: handleSubagentToolPermission,
      subagentToolStart: handleSubagentToolStart,
      subagentToolDone: handleSubagentToolDone,
      subagentClear: handleSubagentClear,
      characterSpritesLoaded: handleCharacterSpritesLoaded,
      floorTilesLoaded: handleFloorTilesLoaded,
      wallTilesLoaded: handleWallTilesLoaded,
      furnitureAssetsLoaded: handleFurnitureAssetsLoaded,
      workspaceFolders: handleWorkspaceFolders,
      settingsLoaded: handleSettingsLoaded,
      externalAssetDirectoriesUpdated: handleExternalAssetDirectoriesUpdated,
      agentTeamInfo: handleAgentTeamInfo,
      agentTokenUsage: handleAgentTokenUsage,
    };

    const handler = (e: MessageEvent) => {
      const msg = e.data as Msg;
      const os = getOfficeState();
      const fn = handlers[msg.type as string];
      if (fn) {
        fn(msg, os);
      } else {
        console.debug('[Webview] Unhandled message type:', msg.type);
      }
    };
    window.addEventListener('message', handler);
    vscode.postMessage({ type: 'webviewReady' });
    return () => window.removeEventListener('message', handler);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [getOfficeState]);

  return {
    agents,
    selectedAgent,
    agentTools,
    agentStatuses,
    subagentTools,
    subagentCharacters,
    layoutReady,
    layoutWasReset,
    loadedAssets,
    workspaceFolders,
    externalAssetDirectories,
    lastSeenVersion,
    extensionVersion,
    watchAllSessions,
    setWatchAllSessions,
    alwaysShowLabels,
    hooksEnabled,
    setHooksEnabled,
    notificationsEnabled,
    setNotificationsEnabled,
    hooksInfoShown,
    noAgents,
    contextWindowMax,
    setContextWindowMaxState,
  };
}
