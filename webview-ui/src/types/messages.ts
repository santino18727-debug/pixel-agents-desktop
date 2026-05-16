// Discriminated union for all messages dispatched via window.postMessage / Tauri agent-event.
// Producer side: Rust file_watcher::translate_to_frontend_messages.
// Consumer side: webview-ui/src/hooks/useExtensionMessages.ts handlers.
export type ExtensionMessage =
  | { type: 'agentStatus'; id: number; status: 'idle' | 'active' | 'waiting' }
  | {
      type: 'agentToolStart';
      id: number;
      toolId: string;
      status: string;
      toolName?: string;
      toolInput?: Record<string, unknown>;
      runInBackground?: boolean;
      permissionActive?: boolean;
    }
  | { type: 'agentToolDone'; id: number; toolId: string }
  | { type: 'agentToolsClear'; id: number }
  | { type: 'agentToolPermission'; id: number }
  | {
      type: 'agentCreated';
      id: number;
      folderName?: string;
      isTeammate?: boolean;
      parentAgentId?: number;
    }
  | { type: 'agentClosed'; id: number }
  | { type: 'agentTokenUsage'; id: number; inputTokens: number; outputTokens: number }
  | {
      type: 'agentTeamInfo';
      id: number;
      isTeamLead: boolean;
      leadAgentId?: number;
      agentName?: string;
    }
  | {
      type: 'existingAgents';
      agents: number[];
      agentMeta: Record<string, unknown>;
      folderNames: Record<number, string>;
    }
  | { type: 'workspaceFolders'; folders: { name: string; path: string }[] }
  | { type: 'noAgents' }
  | { type: 'layoutLoaded'; layout: unknown; wasReset?: boolean }
  | { type: 'characterSpritesLoaded'; characters: unknown }
  | { type: 'floorTilesLoaded'; sprites: unknown }
  | { type: 'wallTilesLoaded'; sets: unknown }
  | { type: 'furnitureAssetsLoaded'; catalog: unknown; sprites: unknown }
  | {
      type: 'settingsLoaded';
      soundEnabled?: boolean;
      watchAllSessions?: boolean;
      alwaysShowLabels?: boolean;
      alwaysOnTop?: boolean;
      extensionVersion?: string;
      lastSeenVersion?: string;
      externalAssetDirectories?: string[];
      maxSessions?: number;
      hooksEnabled?: boolean;
      defaultContextWindowMax?: number;
      notificationsEnabled?: boolean;
      activeSpritePack?: string | null;
    }
  | { type: 'spritePackLoaded'; pack: { name: string; characters: unknown } };
