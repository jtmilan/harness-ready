import React from "react";
import { Link, useLocation } from "react-router-dom";

const LINKS = [
  { path: "/", label: "COMMAND" },
  { path: "/monitoring", label: "MONITORING" },
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
    </div>
  );
}