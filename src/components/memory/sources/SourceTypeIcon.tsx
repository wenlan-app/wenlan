// SPDX-License-Identifier: AGPL-3.0-only
import {
  EnvelopeSimple, File, FilePdf, FileText, Folder, GithubLogo, Globe, GoogleDriveLogo, Image as ImageIcon, NotionLogo, SlackLogo,
} from "@phosphor-icons/react";
import obsidianLogo from "../../../assets/obsidian-logo-gradient.svg";
import type { SourceApp, SourceKind } from "./sourcePresentation";

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
} satisfies Record<SourceKind | SourceApp | "folder", typeof File>;

export function SourceTypeIcon({
  kind,
  size = 20,
  className,
}: {
  kind: SourceKind | SourceApp | "folder" | "obsidian";
  size?: number;
  className?: string;
}) {
  if (kind === "obsidian") {
    return <img alt="" aria-hidden="true" className={className} data-resource-icon={kind}
      src={obsidianLogo} width={size} height={size} style={{ flexShrink: 0, objectFit: "contain" }} />;
  }
  const Icon = ICONS[kind];
  return <Icon aria-hidden="true" className={className} data-resource-icon={kind} focusable="false" size={size} weight="regular" />;
}
