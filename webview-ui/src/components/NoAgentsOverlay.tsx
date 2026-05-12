// Based on pixel-agents by pablodelucca (https://github.com/pablodelucca/pixel-agents)
// Licensed under MIT License
import React from "react";

interface Props {
  visible: boolean;
}

export const NoAgentsOverlay: React.FC<Props> = ({ visible }) => {
  if (!visible) return null;
  return (
    <div
      style={{
        position: "absolute",
        inset: 0,
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        justifyContent: "center",
        background: "rgba(0, 0, 0, 0.65)",
        color: "#ccc",
        fontFamily: "monospace",
        zIndex: 100,
        pointerEvents: "none",
      }}
    >
      <p style={{ fontSize: 15, margin: 0, letterSpacing: "0.02em" }}>
        No active Claude Code sessions found.
      </p>
      <p style={{ fontSize: 12, opacity: 0.55, marginTop: 8 }}>
        Start a Claude Code session — agents will appear here automatically.
      </p>
    </div>
  );
};
