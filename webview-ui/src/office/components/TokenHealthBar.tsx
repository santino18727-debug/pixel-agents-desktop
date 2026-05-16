// Based on pixel-agents by pablodelucca (https://github.com/pablodelucca/pixel-agents)
// Licensed under MIT
import { useEffect, useRef, useState } from 'react';

import {
  CHARACTER_SITTING_OFFSET_PX,
  FUEL_GAUGE_BG,
  FUEL_GAUGE_HEIGHT_PX,
  FUEL_GAUGE_WIDTH_PX,
  TOKEN_BUBBLE_DURATION_MS,
  TOKEN_HEALTH_BAR_DIM_ALPHA,
  TOKEN_HEALTH_BAR_FULL_ALPHA,
  TOKEN_WARN_THRESHOLD,
  TOOL_OVERLAY_VERTICAL_OFFSET,
  getFuelColor,
} from '../../constants.js';
import type { SubagentCharacter } from '../../hooks/useExtensionMessages.js';
import { getContextWindowMax, type OfficeState } from '../engine/officeState.js';
import { CharacterState, TILE_SIZE } from '../types.js';

interface TokenHealthBarProps {
  officeState: OfficeState;
  agents: number[];
  subagentCharacters: SubagentCharacter[];
  containerRef: React.RefObject<HTMLDivElement | null>;
  zoom: number;
  panRef: React.RefObject<{ x: number; y: number }>;
}

/**
 * Always-visible token usage bar above every character. Dimmed when the
 * ratio is low; full-opacity once usage starts approaching the warn level.
 * Also renders a transient speech bubble when an upward threshold (80% / 95%)
 * is crossed — see `OfficeState.setAgentTokens` for the hysteresis logic.
 *
 * Shares the same rAF tick pattern as ToolOverlay; both update at screen
 * refresh independently, but a single bar render is dirt-cheap so the cost
 * of a second loop is negligible (kept consistent with existing code style).
 */
export function TokenHealthBar({
  officeState,
  agents,
  subagentCharacters,
  containerRef,
  zoom,
  panRef,
}: TokenHealthBarProps) {
  const [, setTick] = useState(0);
  useEffect(() => {
    let rafId = 0;
    let lastTick = 0;
    // Throttle to ~15 fps — bar position is just following sprites, no
    // visual animation depends on a 60 fps re-render.
    const TICK_INTERVAL_MS = 66;
    const tick = (now: number) => {
      if (now - lastTick >= TICK_INTERVAL_MS) {
        setTick((n) => n + 1);
        lastTick = now;
      }
      rafId = requestAnimationFrame(tick);
    };
    rafId = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(rafId);
  }, []);

  // Cache container rect; invalidate via ResizeObserver + scroll/resize.
  const rectRef = useRef<DOMRect | null>(null);
  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const update = () => {
      rectRef.current = el.getBoundingClientRect();
    };
    update();
    const ro = new ResizeObserver(update);
    ro.observe(el);
    window.addEventListener('scroll', update, true);
    window.addEventListener('resize', update);
    return () => {
      ro.disconnect();
      window.removeEventListener('scroll', update, true);
      window.removeEventListener('resize', update);
    };
  }, [containerRef]);

  const el = containerRef.current;
  if (!el) return null;
  const rect = rectRef.current ?? el.getBoundingClientRect();
  const dpr = window.devicePixelRatio || 1;
  const canvasW = Math.round(rect.width * dpr);
  const canvasH = Math.round(rect.height * dpr);
  const layout = officeState.getLayout();
  const mapW = layout.cols * TILE_SIZE * zoom;
  const mapH = layout.rows * TILE_SIZE * zoom;
  const deviceOffsetX = Math.floor((canvasW - mapW) / 2) + Math.round(panRef.current.x);
  const deviceOffsetY = Math.floor((canvasH - mapH) / 2) + Math.round(panRef.current.y);

  const allIds = [...agents, ...subagentCharacters.map((s) => s.id)];
  const contextMax = getContextWindowMax();
  const now = Date.now();

  return (
    <>
      {allIds.map((id) => {
        const ch = officeState.characters.get(id);
        if (!ch) return null;
        if (ch.matrixEffect === 'despawn') return null;

        const total = ch.inputTokens + ch.outputTokens;
        if (total <= 0) return null;
        const ratio = contextMax > 0 ? total / contextMax : 0;

        const sittingOffset =
          ch.state === CharacterState.TYPE || ch.state === CharacterState.BREAK
            ? CHARACTER_SITTING_OFFSET_PX
            : 0;
        const screenX = (deviceOffsetX + ch.x * zoom) / dpr;
        const screenY =
          (deviceOffsetY + (ch.y + sittingOffset - TOOL_OVERLAY_VERTICAL_OFFSET) * zoom) / dpr;

        const alpha =
          ratio >= TOKEN_WARN_THRESHOLD ? TOKEN_HEALTH_BAR_FULL_ALPHA : TOKEN_HEALTH_BAR_DIM_ALPHA;

        const bubbleActive =
          ch.tokenBubbleText !== null && now < ch.tokenBubbleExpiresAt;
        const bubbleRemaining = bubbleActive
          ? Math.max(0, ch.tokenBubbleExpiresAt - now)
          : 0;
        // Fade the bubble out over the last 600ms.
        const bubbleAlpha = bubbleActive ? Math.min(1, bubbleRemaining / 600) : 0;

        return (
          <div
            key={id}
            className="absolute flex flex-col items-center -translate-x-1/2"
            style={{
              left: screenX,
              top: screenY - 10,
              pointerEvents: 'none',
              zIndex: 40,
            }}
          >
            {bubbleActive && ch.tokenBubbleText && (
              <div
                className="pixel-panel whitespace-nowrap"
                style={{
                  marginBottom: 2,
                  padding: '2px 6px',
                  fontSize: '14px',
                  opacity: bubbleAlpha,
                  transition: 'opacity 0.2s',
                }}
                title={`${Math.round(ratio * 100)}% of ${contextMax.toLocaleString()} tokens`}
              >
                {ch.tokenBubbleText}
              </div>
            )}
            <div
              style={{
                width: FUEL_GAUGE_WIDTH_PX,
                height: FUEL_GAUGE_HEIGHT_PX,
                background: FUEL_GAUGE_BG,
                opacity: alpha,
              }}
              title={`${Math.round(ratio * 100)}% context used (${(total / 1000).toFixed(0)}k / ${(contextMax / 1000).toFixed(0)}k tokens)`}
            >
              <div
                style={{
                  width: `${Math.min(ratio * 100, 100)}%`,
                  height: '100%',
                  background: getFuelColor(ratio),
                }}
              />
            </div>
          </div>
        );
      })}
    </>
  );
}

// Used implicitly via duration constant import — re-export to keep the
// constant adjacent to the consumer for easier tuning.
export { TOKEN_BUBBLE_DURATION_MS };
