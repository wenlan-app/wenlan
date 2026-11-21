// SPDX-License-Identifier: AGPL-3.0-only
import type { TFunction } from "i18next";
import { resources } from "../../../i18n/resources";
import type { RulesSetupNotice, RulesSetupTool } from "../../../lib/tauri";

const messages = resources.en.translation.rulesSetup.messages;

function isMessageCode(code: string | undefined): code is keyof typeof messages {
  return code !== undefined && Object.prototype.hasOwnProperty.call(messages, code);
}

export function rulesSetupMessage(t: TFunction, code?: string): string {
  return t(`rulesSetup.messages.${isMessageCode(code) ? code : "unknown"}`);
}

export function rulesSetupNotice(t: TFunction, language: string, notice?: RulesSetupNotice): string {
  if (!notice || !isMessageCode(notice.code) || notice.code === "unknown") return rulesSetupMessage(t);
  const message = rulesSetupMessage(t, notice.code);
  const names = notice.tools?.map(rulesSetupName).filter(Boolean) ?? [];
  return names.length ? `${new Intl.ListFormat(language).format(names)} · ${message}` : message;
}

export function rulesSetupName(name?: string): string {
  const label = name?.replaceAll(" / ", " · ");
  return label && !/[\\/\r\n]/u.test(label) ? label : "";
}

export function rulesSetupFileLabel(
  t: TFunction, tool: RulesSetupTool | undefined, path: string,
  missingToolLabel: "rulesSetup.previewFile" | "rulesSetup.sharedSource" = "rulesSetup.previewFile",
): string {
  const name = rulesSetupName(tool?.name) || t(missingToolLabel);
  const parts = path.split(/[\\/]/u);
  // The daemon sends tool paths here. The first item directory determines the kind,
  // even if a skill contains its own commands or a command sits in a nested folder.
  const directory = parts.findIndex((part) => /^(?:skills|commands?|agents?|prompts)$/u.test(part));
  if (directory < 0 || directory === parts.length - 1) return name;

  const suffix = parts[directory - 1] === ".copilot" ? /\.agent\.md$/u : /\.(?:md|toml)$/u;
  const file = parts[parts.length - 1];
  const item = rulesSetupName(parts[directory] === "skills" ? parts[directory + 1]
    : suffix.test(file) ? file.replace(suffix, "") : undefined);
  return item ? `${name} · ${item}` : name;
}

export function rulesSetupSkillSource(source: string): string {
  // GitHub and scoped npm identifiers are labels with two parts.
  return /^@?[\w.-]+(?:\/[\w.-]+)?$/u.test(source) ? source.replace("/", " · ") : "";
}

export function rulesSetupDiffText(line: string, replacement: string): string {
  // Mask path tokens in authored lines as well as labels. A space can be
  // escaped inside a path or included in quotes around the whole path.
  return line.replace(/"[^"\r\n]*[\\/][^"\r\n]*"|'[^'\r\n]*[\\/][^'\r\n]*'|\S*[\\/]\S*/gu, (token) =>
    token === "/" || token === "\\" ? token : replacement);
}
