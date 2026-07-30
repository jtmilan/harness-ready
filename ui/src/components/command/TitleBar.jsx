import React from "react";
import { Link, useLocation } from "react-router-dom";
import SharedStateBadge from "@/components/command/SharedStateBadge";

const LINKS = [
  { path: "/", label: "COMMAND" },
  { path: "/monitoring", label: "MONITORING" },
  { path: "/context", label: "CONTEXT" },
];

export default function TitleBar() {
  const { pathname } = useLocation();
  return (
    <div className="flex items-center px-5 py-2 border-b border-cyan-900/60 bg-[#0A0E13]">
      <span className="font-heading font-bold tracking-[0.35em] text-cyan-300 text-sm">
        AGENT COMMAND CENTER
      </span>
      <nav className="ml-auto flex gap-1">
        {LINKS.map((l) => (
          <Link
            key={l.path}
            to={l.path}
            className={`px-3 py-1 font-heading font-bold tracking-[0.2em] text-xs transition-colors ${
              pathname === l.path
                ? "bg-cyan-400/15 text-cyan-200 border border-cyan-400/60"
                : "text-cyan-700 border border-transparent hover:text-cyan-300"
            }`}
          >
            {l.label}
          </Link>
        ))}
      </nav>
      {/* R-MCP-RW shared-state mode indicator (Phase 7). Non-interactive status
       * chip — NOT a toggle. The dynamic mode value is NEEDS-BACKEND; the chip
       * shows the real gated-OFF default (read-only) until a backend method
       * exists. See docs/REQUIRES-HUMAN-DESIGN-shared-state-write.md. */}
      <div className="ml-3 pl-3 border-l border-cyan-900/50 flex items-center">
        <SharedStateBadge />
      </div>
    </div>
  );
}