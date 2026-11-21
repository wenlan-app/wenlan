// SPDX-License-Identifier: AGPL-3.0-only
import { useTranslation } from "react-i18next";
import { rulesSetupDiffText, rulesSetupMessage } from "./rulesSetupText";

const newlineCode = "no_final_newline";

export function RulesSetupDiff({ diff, codes = [] }: {
  readonly diff: string;
  readonly codes?: string[];
}) {
  const { t } = useTranslation();
  const lines = diff.split("\n");
  const rows: { text: string; change: "added" | "removed" }[] = [];
  let newlineOnly = codes.includes(newlineCode);
  let hasFile = false;
  for (let index = 0; index < lines.length; index++) {
    const line = lines[index];
    if (line.startsWith("--- ") && lines[index + 1]?.startsWith("+++ ")) {
      hasFile = true;
      index++;
      continue;
    }
    if (!hasFile) continue;
    const hunk = /^@@ -\d+,(\d+) \+\d+,(\d+) @@$/u.exec(line);
    if (!hunk) continue;

    let remaining = Number(hunk[1]) + Number(hunk[2]);
    let newlineMarkers = 0;
    const start = rows.length;
    // Use the hunk's line count so authored lines that look like headers stay visible.
    while (index + 1 < lines.length) {
      const next = lines[index + 1];
      if (next === "\\ No newline at end of file") {
        newlineMarkers++;
      } else if (remaining > 0 && (next.startsWith("+") || next.startsWith("-"))) {
        remaining--;
        rows.push({ text: next.slice(1), change: next.startsWith("+") ? "added" : "removed" });
      } else {
        break;
      }
      index++;
    }
    // A final newline change is encoded as the same line removed and added back.
    if (remaining === 0 && hunk[1] === "1" && hunk[2] === "1" && newlineMarkers === 1
      && rows[start].text === rows[start + 1].text) {
      rows.length = start;
      newlineOnly = true;
    }
  }

  const notes = codes.filter((code) => code !== newlineCode).map((code) => rulesSetupMessage(t, code));
  if (!rows.length && !notes.length) {
    if (newlineOnly) notes.push(rulesSetupMessage(t, newlineCode));
    else if (diff.trim()) notes.push(rulesSetupMessage(t));
  }
  return rows.length || notes.length ? (
    <div className="rules-setup-diff" dir="auto">
      {rows.map(({ text, change }, index) => (
        <div key={index} data-change={change} dir="auto">
          {change === "added" ? "+ " : "− "}{rulesSetupDiffText(text, t("rulesSetup.hiddenPath"))}
        </div>
      ))}
      {notes.map((note, index) => <div key={index} dir="auto">{note}</div>)}
    </div>
  ) : null;
}
