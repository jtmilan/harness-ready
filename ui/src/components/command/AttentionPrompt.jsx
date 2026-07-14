import React, { useState } from "react";

export default function AttentionPrompt({ agent, onRespond }) {
  const [value, setValue] = useState("");

  const submit = () => {
    if (!value.trim()) return;
    onRespond(agent.id, value.trim());
    setValue("");
  };

  return (
    <div className="border-t border-amber-500/50 bg-amber-400/10 p-2" onClick={(e) => e.stopPropagation()}>
      <div className="font-mono text-[11px] text-amber-300 mb-1.5">⚠ {agent.attention?.reason || "Agent awaiting input"}</div>
      <input
        value={value}
        onChange={(e) => setValue(e.target.value)}
        onKeyDown={(e) => e.key === "Enter" && submit()}
        placeholder="reply to agent — Enter to send"
        className="w-full bg-[#081019] border border-amber-500/40 text-amber-100 font-mono text-xs px-2 py-1.5 focus:outline-none focus:border-amber-300"
      />
    </div>
  );
}