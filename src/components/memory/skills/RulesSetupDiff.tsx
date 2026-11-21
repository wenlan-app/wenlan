// SPDX-License-Identifier: AGPL-3.0-only
import { useTranslation } from "react-i18next";
import type { RulesSetupTool } from "../../../lib/tauri";
import { rulesSetupDiffText, rulesSetupMessage, rulesSetupName } from "./rulesSetupText";

export function RulesSetupDiff({ diff, tool, tools, codes = [] }: {
  readonly diff: string;
  readonly tool?: RulesSetupTool;
  readonly tools?: Record<string, RulesSetupTool>;
  readonly codes?: string[];
}) {
  const { t } = useTranslation();
  const lines = diff.split("\n");
  const shown: { text: string; change?: "added" | "removed" }[] = [];
  let remaining = 0;
  let hasFile = false;
  for (let index = 0; index < lines.length; index++) {
    const line = lines[index];
    if (remaining === 0 && line.startsWith("--- ") && lines[index + 1]?.startsWith("+++ ")) {
      const path = line.slice(4);
      const name = rulesSetupName((tool ?? tools?.[path] ?? tools?.[path.replaceAll("/", "\\")])?.name);
      shown.push({ text: name || t("rulesSetup.previewFile") });
      hasFile = true;
      index++;
      continue;
    }
    if (!hasFile) continue;
    const hunk = /^@@ -\d+,(\d+) \+\d+,(\d+) @@$/u.exec(line);
    if (hunk && remaining === 0) {
      remaining = Number(hunk[1]) + Number(hunk[2]);
      shown.push({ text: line });
    } else if (line === "\\ No newline at end of file") {
      shown.push({ text: rulesSetupMessage(t, "no_final_newline") });
    } else if (remaining > 0 && (line.startsWith("+") || line.startsWith("-"))) {
      remaining--;
      shown.push({
        text: line[0] + rulesSetupDiffText(line.slice(1), t("rulesSetup.hiddenPath")),
        change: line.startsWith("+") ? "added" : "removed",
      });
    }
  }

  for (const code of codes) shown.push({ text: rulesSetupMessage(t, code) });
  if (!shown.length && diff.trim()) shown.push({ text: rulesSetupMessage(t) });
  return shown.length ? (
    <pre className="rules-setup-diff" dir="auto">
      {shown.map(({ text, change }, index) => (
        <span key={index} data-change={change}>{text}{"\n"}</span>
      ))}
    </pre>
  ) : null;
}
