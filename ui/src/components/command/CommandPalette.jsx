import React, { useEffect, useRef } from "react";
import { X } from "lucide-react";
import {
  Command,
  CommandInput,
  CommandList,
  CommandEmpty,
  CommandGroup,
  CommandItem,
} from "@/components/ui/command";

// CommandPalette — ⌘K command palette built on the existing shadcn Command
// (cmdk) for free a11y: listbox/option roles, keyboard nav, active-descendant,
// and case-insensitive filtering all come from cmdk. Styled to match the
// terminal aesthetic (bg-[#0A1219], cyan borders, font-mono/heading) per the
// existing overlay idiom (see TemplatesOverlay).
//
// Props:
//   open      — boolean; palette is unmounted when false (matches the
//               conditional render in Home.jsx).
//   onClose   — called on Escape, backdrop click, or after a command runs.
//   commands  — array of { id, label, keywords?, hint?, run }.

/**
 * @param {{ open: boolean, onClose: () => void, commands: Array<{id: string, label: string, keywords?: string[], hint?: string, run: () => void}> }} props
 */
export default function CommandPalette({ open, onClose, commands }) {
  const panelRef = useRef(null);

  // Focus the panel on open so keydown events land immediately. cmdk's
  // CommandInput renders an <input> that cmdk auto-focuses, but we keep a
  // belt-and-suspenders focus on the panel for the Escape listener.
  useEffect(() => {
    if (open) {
      // A microtask delay lets cmdk mount its input before we try to focus.
      // Without this, the input's own autoFocus wins and our focus call is a
      // no-op — which is actually fine, but we want the panel to be the
      // keydown target for Escape.
      const id = requestAnimationFrame(() => {
        panelRef.current?.focus();
      });
      return () => cancelAnimationFrame(id);
    }
    return undefined;
  }, [open]);

  if (!open) return null;

  return (
    <div
      className="fixed inset-0 z-50 flex items-start justify-center pt-[18vh] bg-black/70 backdrop-blur-sm"
      onClick={onClose}
      aria-label="Command palette backdrop"
    >
      <div
        ref={panelRef}
        tabIndex={-1}
        role="dialog"
        aria-modal="true"
        aria-label="Command palette"
        className="w-full max-w-xl mx-4 border-2 border-cyan-400/60 bg-[#0A1219] shadow-[0_0_30px_rgba(0,229,255,0.25)] rounded-md overflow-hidden outline-none"
        onClick={(e) => e.stopPropagation()}
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            e.preventDefault();
            e.stopPropagation();
            onClose();
          }
        }}
      >
        {/* Header bar — matches TemplatesOverlay's title strip. */}
        <div className="flex items-center gap-3 px-4 py-2 border-b border-cyan-800/70 bg-[#0C1720]">
          <span className="font-heading font-bold tracking-[0.25em] text-cyan-300 text-xs">
            COMMAND PALETTE
          </span>
          <span className="ml-auto font-mono text-[10px] text-cyan-700 select-none">
            ⌘K
          </span>
          <button
            onClick={onClose}
            className="text-cyan-600 hover:text-cyan-300 transition-colors"
            aria-label="Close command palette"
          >
            <X className="w-4 h-4" />
          </button>
        </div>

        {/* cmdk Command root: owns the input, list, filter, and a11y tree. */}
        <Command className="bg-transparent text-cyan-100 [&_[cmdk-item]]:px-0 [&_[cmdk-item]]:py-0">
          <div className="flex items-center border-b border-cyan-900/60 px-3">
            <CommandInput
              placeholder="Type a command…"
              autoFocus
              className="flex h-10 w-full bg-transparent py-3 font-mono text-sm text-cyan-100 outline-none placeholder:text-cyan-700"
            />
          </div>
          <CommandList className="max-h-[320px] overflow-y-auto">
            <CommandEmpty className="py-8 text-center text-sm text-cyan-700 font-mono">
              No matching commands
            </CommandEmpty>
            <CommandGroup className="py-1">
              {commands.map((cmd) => (
                <CommandItem
                  key={cmd.id}
                  // cmdk filters on the item `value`; fold keywords in so synonym search
                  // works (the shadcn wrapper spreads unknown props to the DOM, so we do NOT
                  // pass a raw `keywords` prop — that would warn + be ignored).
                  value={`${cmd.label} ${(cmd.keywords || []).join(" ")}`}
                  onSelect={() => {
                    cmd.run();
                    onClose();
                  }}
                  className="relative flex cursor-default select-none items-center gap-2 px-4 py-2 font-mono text-sm text-cyan-200 outline-none data-[selected=true]:bg-cyan-400/15 data-[selected=true]:text-cyan-100 rounded-none"
                >
                  <span className="flex-1 truncate">{cmd.label}</span>
                  {cmd.hint && (
                    <span className="ml-auto shrink-0 text-xs text-cyan-700 tracking-wider font-mono">
                      {cmd.hint}
                    </span>
                  )}
                </CommandItem>
              ))}
            </CommandGroup>
          </CommandList>
        </Command>
      </div>
    </div>
  );
}
