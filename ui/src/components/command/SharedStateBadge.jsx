/**
 * SharedStateBadge — non-interactive status chip for the R-MCP-RW shared-state
 * mode indicator (Phase 7).
 *
 * HARD RULES enforced here:
 *   R3: STATUS indicator, NOT a toggle. Rendered as a <span>, no onClick, no
 *       button role, no hover affordance that implies user control over the mode.
 *   R5: Does NOT use a sacred health lane (--need/--danger). Cyan/muted for
 *       read-only (the real gated-OFF default), distinct amber for read-write.
 *   R8: Accessible name via aria-label + title; non-interactive so not focusable.
 *   R10: Does NOT import or extend agentBridge. The dynamic mode value is
 *        NEEDS-BACKEND — see proposed contract below.
 *
 * PROPOSED BRIDGE CONTRACT (NEEDS-BACKEND — NOT implemented in Phase 7):
 *
 *   // tauriAgentBridge.js + MockAgentBridge (interface parity):
 *   //   getSharedStateMode(): Promise<'read-only' | 'read-write'>
 *   //
 *   // - Resolves the current shared-state gate state from the backend.
 *   // - 'read-only'  = write gate closed (default; R-MCP-RW gated-OFF).
 *   // - 'read-write' = write gate open after explicit operator arm + confirm.
 *   // - Never throws; on backend error resolves to 'read-only' (safe default).
 *   //
 *   // A future arm/disarm control MUST be gated + confirmed (never a silent
 *   // toggle) and is out of scope for this indicator. See
 *   // docs/REQUIRES-HUMAN-DESIGN-shared-state-write.md.
 *
 * Until that bridge method exists, this badge calls sharedStateLabel(undefined)
 * and honestly renders the KNOWN DEFAULT (read-only), labeled NEEDS-BACKEND in
 * the accessible description.
 */
import React from "react";
import { Lock, Unlock } from "lucide-react";
import { sharedStateLabel } from "@/lib/sharedState";

const ACCESSIBLE_DESCRIPTION =
  "Read-write shared state is gated-OFF by default (R-MCP-RW); " +
  "this indicator shows the current mode. The dynamic mode value is " +
  "NEEDS-BACKEND — proposed bridge.getSharedStateMode(): " +
  "Promise<'read-only' | 'read-write'>.";

const TONE_STYLES = {
  readonly: {
    className:
      "inline-flex items-center gap-1.5 px-2 py-0.5 " +
      "border border-cyan-900/70 text-cyan-600 " +
      "font-heading font-bold tracking-[0.2em] text-[10px] " +
      "bg-cyan-950/20",
    Icon: Lock,
  },
  readwrite: {
    // Distinct but non-health styling (R5): amber, not --need/--danger.
    className:
      "inline-flex items-center gap-1.5 px-2 py-0.5 " +
      "border border-amber-500/40 text-amber-300/80 " +
      "font-heading font-bold tracking-[0.2em] text-[10px] " +
      "bg-amber-950/20",
    Icon: Unlock,
  },
  unknown: {
    // Fallback matches readonly (safe default) — same visual, distinct tone key
    // in case a future consumer branches on tone.
    className:
      "inline-flex items-center gap-1.5 px-2 py-0.5 " +
      "border border-cyan-900/70 text-cyan-600 " +
      "font-heading font-bold tracking-[0.2em] text-[10px] " +
      "bg-cyan-950/20",
    Icon: Lock,
  },
};

/**
 * Non-interactive shared-state mode chip.
 *
 * @param {{ mode?: unknown }} props — `mode` is the (future) backend value.
 *   Omit / pass undefined today; the badge shows the read-only default.
 */
export default function SharedStateBadge({ mode } = {}) {
  const { text, tone } = sharedStateLabel(mode);
  const style = TONE_STYLES[tone] ?? TONE_STYLES.unknown;
  const Icon = style.Icon;

  return (
    <span
      role="status"
      aria-label={ACCESSIBLE_DESCRIPTION}
      title={ACCESSIBLE_DESCRIPTION}
      className={style.className}
    >
      <Icon size={11} aria-hidden="true" />
      <span>{text}</span>
      <span className="text-[9px] opacity-60 tracking-[0.15em]">
        NEEDS-BACKEND
      </span>
    </span>
  );
}
