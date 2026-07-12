// Shared positioning logic for absolute-positioned overlays that track the
// pixel-art canvas (ToolOverlay, TokenHealthBar). Consolidates the rAF tick
// throttle, ResizeObserver-cached rect, and map-to-screen offset math that
// was duplicated across both components.
import { useEffect, useRef, useState, type RefObject } from 'react';

export interface DeviceOffsets {
  deviceOffsetX: number;
  deviceOffsetY: number;
  canvasW: number;
  canvasH: number;
}

export interface OverlayPositioning {
  /** Increments at ~tickIntervalMs to trigger re-renders that follow sprite motion. */
  tick: number;
  /** Cached container DOMRect (invalidated by ResizeObserver + scroll + resize). */
  getRect: () => DOMRect | null;
  /** Compute the centered map-origin offset in device pixels, including pan. */
  getDeviceOffsets: (
    dpr: number,
    zoom: number,
    layout: { cols: number; rows: number },
    panX: number,
    panY: number,
    tileSize: number,
  ) => DeviceOffsets;
}

export function useOverlayPositioning(
  containerRef: RefObject<HTMLElement | null>,
  { active = true, tickIntervalMs = 66 }: { active?: boolean; tickIntervalMs?: number } = {},
): OverlayPositioning {
  // rAF throttled tick — re-renders the overlay so it tracks sprite positions
  // without forcing a 60fps cycle (overlays are static labels above sprites
  // that animate independently on the canvas).
  //
  // The loop is gated on `active` (are there any characters to track?) and on
  // document visibility, so an idle or minimized app doesn't burn CPU/battery
  // spinning a re-render loop with nothing to position. It resumes on the next
  // visibilitychange or when characters appear.
  const [tick, setTick] = useState(0);
  useEffect(() => {
    if (!active) return;
    let rafId = 0;
    let lastTick = 0;
    const loop = (now: number) => {
      if (now - lastTick >= tickIntervalMs) {
        setTick((n) => n + 1);
        lastTick = now;
      }
      rafId = requestAnimationFrame(loop);
    };
    const start = () => {
      if (rafId === 0 && !document.hidden) rafId = requestAnimationFrame(loop);
    };
    const stop = () => {
      if (rafId !== 0) {
        cancelAnimationFrame(rafId);
        rafId = 0;
      }
    };
    const onVisibility = () => (document.hidden ? stop() : start());
    document.addEventListener('visibilitychange', onVisibility);
    start();
    return () => {
      document.removeEventListener('visibilitychange', onVisibility);
      stop();
    };
  }, [active, tickIntervalMs]);

  // Cached rect via ResizeObserver — avoids a synchronous reflow per tick.
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

  return {
    tick,
    getRect: () => rectRef.current,
    getDeviceOffsets: (dpr, zoom, layout, panX, panY, tileSize) => {
      const rect = rectRef.current;
      if (!rect) return { deviceOffsetX: 0, deviceOffsetY: 0, canvasW: 0, canvasH: 0 };
      const canvasW = Math.round(rect.width * dpr);
      const canvasH = Math.round(rect.height * dpr);
      const mapW = layout.cols * tileSize * zoom;
      const mapH = layout.rows * tileSize * zoom;
      return {
        canvasW,
        canvasH,
        deviceOffsetX: Math.floor((canvasW - mapW) / 2) + Math.round(panX),
        deviceOffsetY: Math.floor((canvasH - mapH) / 2) + Math.round(panY),
      };
    },
  };
}
