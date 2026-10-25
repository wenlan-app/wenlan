// SPDX-License-Identifier: AGPL-3.0-only
import {
  EnvelopeSimple, File, FilePdf, FileText, Folder, GithubLogo, Globe, GoogleDriveLogo, Image as ImageIcon, InstagramLogo, NotionLogo,
  SlackLogo, ThreadsLogo, XLogo,
} from "@phosphor-icons/react";
import logseqLogo from "../../../assets/logseq-logo.svg";
import obsidianLogo from "../../../assets/obsidian-logo-gradient.svg";
import xiaohongshuLogo from "../../../assets/xiaohongshu-logo.svg";
import type { NoteApp, SourceApp, SourceKind } from "./sourcePresentation";

const ICONS = {
  pdf: FilePdf,
  text: FileText,
  image: ImageIcon,
  webpage: Globe,
  file: File,
  folder: Folder,
  // Phosphor has no Gmail mark, so mail uses its envelope.
  gmail: EnvelopeSimple,
  googleDrive: GoogleDriveLogo,
  notion: NotionLogo,
  slack: SlackLogo,
  github: GithubLogo,
  x: XLogo,
  threads: ThreadsLogo,
  instagram: InstagramLogo,
} satisfies Record<Exclude<SourceKind | SourceApp | "folder", "xiaohongshu">, typeof File>;

// Phosphor has no RedNote mark; its logo is a red badge.
const LOGOS = { obsidian: obsidianLogo, logseq: logseqLogo, xiaohongshu: xiaohongshuLogo } as const;

export function SourceTypeIcon({
  kind,
  size = 20,
  className,
}: {
  kind: SourceKind | SourceApp | "folder" | NoteApp;
  size?: number;
  className?: string;
}) {
  if (kind === "obsidian" || kind === "logseq" || kind === "xiaohongshu") {
    return <img alt="" aria-hidden="true" className={className} data-resource-icon={kind}
      src={LOGOS[kind]} width={size} height={size} style={{ flexShrink: 0, objectFit: "contain" }} />;
  }
  const Icon = ICONS[kind];
  return <Icon aria-hidden="true" className={className} data-resource-icon={kind} focusable="false" size={size} weight="regular" />;
}
