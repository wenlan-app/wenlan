// SPDX-License-Identifier: AGPL-3.0-only
import { ReferencePreview, referenceExcerpt } from "../links/ReferencePreview";
import type { ReferencePreviewRequest } from "../links/referenceTypes";
import type { Page } from "../../../lib/tauri";
import { readablePageContent } from "./original/originalContent";

export interface PageLinkPreviewRequest {
  pageId: string;
  anchor: HTMLAnchorElement;
  keyboard: boolean;
}

interface PageLinkPreviewProps {
  request: PageLinkPreviewRequest | null;
  onDismiss(): void;
  onOpen(pageId: string): void;
  onPointerEnter(): void;
  onPointerLeave(): void;
}

export default function PageLinkPreview({ request, onDismiss, onOpen, onPointerEnter, onPointerLeave }: PageLinkPreviewProps) {
  const sharedRequest: ReferencePreviewRequest | null = request ? {
    target: { kind: "page", id: request.pageId },
    anchor: request.anchor,
    keyboard: request.keyboard,
  } : null;
  return (
    <ReferencePreview
      request={sharedRequest}
      onDismiss={onDismiss}
      onOpen={(target) => { if (target.kind === "page") { onDismiss(); onOpen(target.id); } }}
      onPointerEnter={onPointerEnter}
      onPointerLeave={onPointerLeave}
      role="dialog"
      dataPageLinkPreview
      copyPrefix="pageLinkPreview"
    />
  );
}

export function pageLinkPreviewExcerpt(page: Pick<Page, "summary" | "content" | "creation_kind">): string {
  return referenceExcerpt(page.summary || readablePageContent(page));
}
