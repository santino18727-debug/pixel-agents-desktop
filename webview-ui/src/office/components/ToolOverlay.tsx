// Based on pixel-agents by pablodelucca (https://github.com/pablodelucca/pixel-agents)
// Licensed under MIT
import { Button } from '../../components/ui/Button.js';
import {
  CHARACTER_SITTING_OFFSET_PX,
  TEAM_LEAD_COLOR,
  TEAM_ROLE_COLOR,
  TOOL_OVERLAY_VERTICAL_OFFSET,
} from '../../constants.js';
import type { SubagentCharacter } from '../../hooks/useExtensionMessages.js';
import type { OfficeState } from '../engine/officeState.js';
import type { ToolActivity } from '../types.js';
import { CharacterState, TILE_SIZE } from '../types.js';
import { useOverlayPositioning } from '../utils/overlayPositioning.js';

interface ToolOverlayProps {
  officeState: OfficeState;
  agents: number[];
  agentTools: Record<number, ToolActivity[]>;
  subagentCharacters: SubagentCharacter[];
  containerRef: React.RefObject<HTMLDivElement | null>;
  zoom: number;
  panRef: React.RefObject<{ x: number; y: number }>;
  onCloseAgent: (id: number) => void;
  alwaysShowOverlay: boolean;
}

/** Derive a short human-readable activity string from tools/status */
function getActivityText(
  agentId: number,
  agentTools: Record<number, ToolActivity[]>,
  isActive: boolean,
): string {
  const tools = agentTools[agentId];
  if (tools && tools.length > 0) {
    // Find the latest non-done tool
    const activeTool = [...tools].reverse().find((t) => !t.done);
    if (activeTool) {
      if (activeTool.permissionWait) return 'Needs approval';
      return activeTool.status;
    }
    // All tools done but agent still active (mid-turn) — keep showing last tool status
    if (isActive) {
      const lastTool = tools[tools.length - 1];
      if (lastTool) return lastTool.status;
    }
  }

  // No tool history (or cleared by turn_duration) but agent is still generating text
  // between tools or before the first ToolUse. Avoid the misleading "Idle" label.
  if (isActive) return 'Thinking…';

  return 'Idle';
}

export function ToolOverlay({
  officeState,
  agents,
  agentTools,
  subagentCharacters,
  containerRef,
  zoom,
  panRef,
  onCloseAgent,
  alwaysShowOverlay,
}: ToolOverlayProps) {
  // Shared rAF tick + cached-rect positioning helper.
  const { getRect, getDeviceOffsets } = useOverlayPositioning(containerRef);

  const el = containerRef.current;
  if (!el) return null;
  // Fall back to a live getBoundingClientRect on first paint before the
  // ResizeObserver has populated the cache; subsequent ticks use the cache.
  const rect = getRect() ?? el.getBoundingClientRect();
  const dpr = window.devicePixelRatio || 1;
  const layout = officeState.getLayout();
  // Mirror the cached-rect branch when falling back to the live rect.
  let canvasW: number;
  let canvasH: number;
  let deviceOffsetX: number;
  let deviceOffsetY: number;
  if (getRect()) {
    const off = getDeviceOffsets(dpr, zoom, layout, panRef.current.x, panRef.current.y, TILE_SIZE);
    canvasW = off.canvasW;
    canvasH = off.canvasH;
    deviceOffsetX = off.deviceOffsetX;
    deviceOffsetY = off.deviceOffsetY;
  } else {
    canvasW = Math.round(rect.width * dpr);
    canvasH = Math.round(rect.height * dpr);
    const mapW = layout.cols * TILE_SIZE * zoom;
    const mapH = layout.rows * TILE_SIZE * zoom;
    deviceOffsetX = Math.floor((canvasW - mapW) / 2) + Math.round(panRef.current.x);
    deviceOffsetY = Math.floor((canvasH - mapH) / 2) + Math.round(panRef.current.y);
  }
  // Silence noUnusedLocals — canvasW/H are kept for parity with the original
  // computation (future overlays may need bounds clamping).
  void canvasW;
  void canvasH;

  const selectedId = officeState.selectedAgentId;
  const hoveredId = officeState.hoveredAgentId;

  // All character IDs
  const allIds = [...agents, ...subagentCharacters.map((s) => s.id)];

  return (
    <>
      {allIds.map((id) => {
        const ch = officeState.characters.get(id);
        if (!ch) return null;

        const isSelected = selectedId === id;
        const isHovered = hoveredId === id;
        const isSub = ch.isSubagent;

        // Only show for hovered or selected agents (unless always-show is on)
        if (!alwaysShowOverlay && !isSelected && !isHovered) return null;

        // Position above character
        const sittingOffset = (ch.state === CharacterState.TYPE || ch.state === CharacterState.BREAK) ? CHARACTER_SITTING_OFFSET_PX : 0;
        const screenX = (deviceOffsetX + ch.x * zoom) / dpr;
        const screenY =
          (deviceOffsetY + (ch.y + sittingOffset - TOOL_OVERLAY_VERTICAL_OFFSET) * zoom) / dpr;

        // Get activity text
        const subHasPermission = isSub && ch.bubbleType === 'permission';
        let activityText: string;
        if (isSub) {
          if (subHasPermission) {
            activityText = 'Needs approval';
          } else {
            const sub = subagentCharacters.find((s) => s.id === id);
            activityText = sub ? sub.label : 'Subtask';
          }
        } else {
          activityText = getActivityText(id, agentTools, ch.isActive);
        }

        // Determine dot color
        const tools = agentTools[id];
        const hasPermission = subHasPermission || tools?.some((t) => t.permissionWait && !t.done);
        const hasActiveTools = tools?.some((t) => !t.done);
        const isActive = ch.isActive;

        let dotColor: string | null = null;
        if (hasPermission) {
          dotColor = 'var(--color-status-permission)';
        } else if (isActive && hasActiveTools) {
          dotColor = 'var(--color-status-active)';
        }

        // Team info
        const teamRoleLabel = ch.isTeamLead ? 'LEAD' : ch.agentName || null;
        const hasExtraLines = !!(ch.folderName || teamRoleLabel);

        return (
          <div
            key={id}
            className="absolute flex flex-col items-center -translate-x-1/2"
            style={{
              left: screenX,
              top: screenY - (hasExtraLines ? 34 : 28),
              pointerEvents: isSelected ? 'auto' : 'none',
              opacity: alwaysShowOverlay && !isSelected && !isHovered ? (isSub ? 0.5 : 0.75) : 1,
              zIndex: isSelected ? 42 : 41,
            }}
          >
            <div className="flex items-center border-border px-8 pt-2 pb-4 gap-5 pixel-panel whitespace-nowrap max-w-2xs">
              {dotColor && (
                <span
                  className={`w-6 h-6 rounded-full shrink-0 ${isActive && !hasPermission ? 'pixel-pulse' : ''}`}
                  style={{ background: dotColor }}
                />
              )}
              <div className="flex flex-col gap-0 overflow-hidden">
                {teamRoleLabel && (
                  <span
                    className="overflow-hidden text-ellipsis block leading-none"
                    style={{
                      fontSize: '18px',
                      color: ch.isTeamLead ? TEAM_LEAD_COLOR : TEAM_ROLE_COLOR,
                      fontWeight: ch.isTeamLead ? 'bold' : undefined,
                    }}
                  >
                    {teamRoleLabel}
                  </span>
                )}
                <span
                  className="overflow-hidden text-ellipsis block leading-none"
                  style={{
                    fontSize: isSub ? '20px' : '22px',
                    fontStyle: isSub ? 'italic' : undefined,
                  }}
                >
                  {activityText}
                </span>
                {ch.folderName && (
                  <span className="text-2xs leading-none overflow-hidden text-ellipsis block">
                    {ch.folderName}
                  </span>
                )}
              </div>
              {isSelected && !isSub && (
                <Button
                  variant="ghost"
                  size="icon"
                  onClick={(e) => {
                    e.stopPropagation();
                    onCloseAgent(id);
                  }}
                  title="Close agent"
                  className="ml-2 shrink-0 leading-none"
                >
                  ×
                </Button>
              )}
            </div>
          </div>
        );
      })}
    </>
  );
}
