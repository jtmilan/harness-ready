// Skills.jsx — R-SKILLS-OSS: read-only catalog of the repo's .claude/skills.
// NOTE: This is a static build-time snapshot of the repo's .claude/skills (R-SKILLS-OSS);
// no network calls are made. Regenerate ui/src/data/skills-raw.json when skills change.
// R4: never invent skills not in the manifest.

import React, { useMemo } from "react";
import TitleBar from "@/components/command/TitleBar";
import { buildSkillsManifest } from "@/lib/skillsManifest";
import raw from "@/data/skills-raw.json";

export default function Skills() {
  const skills = useMemo(() => buildSkillsManifest(raw), []);

  return (
    <div className="h-screen flex flex-col bg-[#0D1117] scanlines overflow-hidden">
      <TitleBar />
      <div className="flex-1 overflow-y-auto terminal-scroll p-5 space-y-4">
        {/* Header */}
        <div className="border-b border-cyan-900/60 pb-3">
          <h1 className="font-heading text-[13px] tracking-[0.3em] text-cyan-400 font-bold">
            SKILLS CATALOG
          </h1>
          <p className="font-mono text-[11px] text-cyan-600 mt-1">
            // static snapshot of .claude/skills — {skills.length} skill{skills.length === 1 ? "" : "s"} published
          </p>
        </div>

        {skills.length === 0 ? (
          // Honest empty state (R3/R4): no skills exist in this repo's .claude/skills yet.
          <div className="border border-cyan-900/60 bg-[#0A1219] rounded-sm px-5 py-6">
            <p className="font-mono text-[11px] text-cyan-700 leading-relaxed">
              // no skills published in this repo yet — add a SKILL.md under .claude/skills/ and commit a regenerated ui/src/data/skills-raw.json to populate this catalog.
            </p>
          </div>
        ) : (
          <div className="space-y-3">
            {skills.map((skill) => (
              <div
                key={skill.id}
                className="border border-cyan-900/60 bg-[#0A1219] rounded-sm px-4 py-3"
              >
                <h2 className="font-heading text-[12px] tracking-[0.2em] text-cyan-300 font-bold">
                  {skill.name}
                </h2>
                {skill.description && (
                  <p className="font-mono text-[11px] text-cyan-600 mt-1 leading-relaxed">
                    {skill.description}
                  </p>
                )}
                <p className="font-mono text-[10px] text-cyan-700 mt-1.5">
                  {skill.source}
                </p>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
