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

export function rulesSetupFileLabel(t: TFunction, tool: RulesSetupTool | undefined, path: string): string {
  const name = rulesSetupName(tool?.name);
  const file = rulesSetupName(path.split(/[\\/]/u).pop());
  return name && file ? `${name} · ${file}` : name || t("rulesSetup.previewFile");
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
