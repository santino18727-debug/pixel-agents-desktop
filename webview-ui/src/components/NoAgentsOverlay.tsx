// Based on pixel-agents by pablodelucca (https://github.com/pablodelucca/pixel-agents)
// Licensed under MIT License
import React from 'react';

interface Props {
  visible: boolean;
}

/**
 * First-run / idle empty state. Rendered when no Claude Code sessions are
 * active. Uses the app's pixel design system (pixel font, `pixel-panel` card,
 * hard shadow) so it reads as part of the product, and points the user at the
 * "+ Agent" button in the bottom toolbar — the fastest path to a first agent.
 *
 * z-index sits below the modal layer (Modal uses 50/51) so the card never
 * paints over an open Settings/Changelog dialog, but above the office canvas
 * and its overlays. pointer-events stay off so the toolbar underneath the
 * lightened scrim remains clickable.
 */
export const NoAgentsOverlay: React.FC<Props> = ({ visible }) => {
  if (!visible) return null;
  return (
    <div
      className="absolute inset-0 z-30 flex items-center justify-center pointer-events-none"
      style={{ background: 'rgba(10, 10, 20, 0.45)' }}
      role="status"
      aria-live="polite"
    >
      <div className="pixel-panel px-16 py-14 max-w-2xl text-center">
        <div className="text-4xl mb-6 leading-none" aria-hidden="true">
          ‹∅›
        </div>
        <p className="text-base text-text m-0">No active Claude Code sessions found.</p>
        <p className="text-xs text-text-muted mt-6 mb-0 leading-normal">
          Click <span className="text-accent-bright">+ Agent</span> below to launch one — or start
          Claude Code in any terminal and your agents appear here automatically.
        </p>
      </div>
    </div>
  );
};
