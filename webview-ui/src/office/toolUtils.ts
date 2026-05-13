// Based on pixel-agents by pablodelucca (https://github.com/pablodelucca/pixel-agents)
// Licensed under MIT
/** Map status prefixes back to tool names for animation selection */
const STATUS_TO_TOOL: Record<string, string> = {
  Reading: 'Read',
  Searching: 'Grep',
  Globbing: 'Glob',
  Fetching: 'WebFetch',
  'Searching web': 'WebSearch',
  Writing: 'Write',
  Editing: 'Edit',
  Running: 'Bash',
  Task: 'Task',
};

export function extractToolName(status: string): string | null {
  for (const [prefix, tool] of Object.entries(STATUS_TO_TOOL)) {
    if (status.startsWith(prefix)) return tool;
  }
  const first = status.split(/[\s:]/)[0];
  return first || null;
}

import { ZOOM_DEFAULT_DPR_FACTOR, ZOOM_MIN } from '../constants.js';

/** Compute a default integer zoom level (device pixels per sprite pixel) */
export function defaultZoom(): number {
  const dpr = window.devicePixelRatio || 1;
  return Math.max(ZOOM_MIN, Math.round(ZOOM_DEFAULT_DPR_FACTOR * dpr));
}

/** Extract a short filename from a path string */
function basename(p: string): string {
  return p.replace(/\\/g, '/').split('/').pop() ?? p;
}

/** Truncate a string to maxLen chars, appending ellipsis if needed */
function truncate(s: string, maxLen: number): string {
  return s.length <= maxLen ? s : s.slice(0, maxLen) + '…';
}

/**
 * Produce a human-readable activity label from tool name and optional input.
 * toolInput is the raw JSON input object from the agent event.
 */
export function enrichToolStatus(toolName: string, toolInput?: Record<string, unknown>): string {
  switch (toolName) {
    case 'Read': {
      const fp = toolInput?.file_path as string | undefined;
      return fp ? `Reading ${basename(fp)}` : 'Reading file…';
    }
    case 'Edit': {
      const fp = toolInput?.path as string | undefined;
      return fp ? `Editing ${basename(fp)}` : 'Editing file…';
    }
    case 'Write': {
      const fp = toolInput?.path as string | undefined;
      return fp ? `Writing ${basename(fp)}` : 'Writing file…';
    }
    case 'Bash': {
      const cmd = toolInput?.command as string | undefined;
      return cmd ? `Running: ${truncate(cmd, 30)}` : 'Running command…';
    }
    case 'Glob':
      return 'Searching files…';
    case 'Grep':
      return 'Searching code…';
    case 'WebFetch':
      return 'Fetching web…';
    case 'WebSearch':
      return 'Searching web…';
    case 'Task':
    case 'Agent': {
      const desc = toolInput?.description as string | undefined;
      return desc ? `Subtask: ${truncate(desc, 40)}` : 'Subtask…';
    }
    case 'AskUserQuestion':
      return 'Waiting for answer…';
    case 'EnterPlanMode':
      return 'Planning…';
    default:
      return toolName ? `Using ${toolName}` : 'Working…';
  }
}
