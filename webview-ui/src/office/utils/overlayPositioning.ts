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
  tickIntervalMs = 66,
): OverlayPositioning {
  // rAF throttled tick — re-renders the overlay so it tracks sprite positions
  // without forcing a 60fps cycle (overlays are static labels above sprites
  // that animate independently on the canvas).
  const [tick, setTick] = useState(0);
  useEffect(() => {
    let rafId = 0;
    let lastTick = 0;
    const loop = (now: number) => {
      if (now - lastTick >= tickIntervalMs) {
        setTick((n) => n + 1);
        lastTick = now;
      }
      rafId = requestAnimationFrame(loop);
    };
    rafId = requestAnimationFrame(loop);
    return () => cancelAnimationFrame(rafId);
  }, [tickIntervalMs]);

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
