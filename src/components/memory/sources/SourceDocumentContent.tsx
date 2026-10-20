// SPDX-License-Identifier: AGPL-3.0-only
import type { ReactNode } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { ReferenceLink } from "../links/ReferenceLink";
import { referenceTargetFromHref } from "../links/referenceTypes";
import { displayUrl, isBareUrlLink } from "../../../lib/displayUrl";
import "./SourceDocumentContent.css";

interface SourceDocumentContentProps {
  content: string;
  title: string;
}

type MarkdownNode = {
  type: string;
  depth?: number;
  value?: string;
  alt?: string;
  children?: MarkdownNode[];
};

type MarkdownRoot = { type: "root"; children: MarkdownNode[] };

function normalizeHeadingText(value: string): string {
  return value.normalize("NFC").trim().replace(/\s+/gu, " ");
}

function headingText(node: MarkdownNode): string {
  if (node.type === "text" || node.type === "inlineCode") return node.value ?? "";
  if (node.type === "image" || node.type === "imageReference") return node.alt ?? "";
  if (node.type === "break") return " ";
  return node.children?.map(headingText).join("") ?? "";
}

function omitMatchingLeadingTitle(title: string) {
  const normalizedTitle = normalizeHeadingText(title);
  return () => (tree: MarkdownRoot) => {
    if (!normalizedTitle) return;
    const first = tree.children[0];
    if (!first) return;
    if (
      first.type === "heading" &&
      first.depth === 1 &&
      normalizeHeadingText(headingText(first)) === normalizedTitle
    ) {
      tree.children.shift();
    }
  };
}

const markdownComponents = {
  p: ({ children }: { children?: ReactNode }) => <p>{children}</p>,
  strong: ({ children }: { children?: ReactNode }) => <strong>{children}</strong>,
  a: ({ href, children }: { href?: string; children?: ReactNode }) =>
    referenceTargetFromHref(href) ? (
      <ReferenceLink href={href!}>{children}</ReferenceLink>
    ) : (
      <a
        href={href}
        target={href?.startsWith("#") ? undefined : "_blank"}
        rel="noopener noreferrer"
        title={isBareUrlLink(href, children) ? href : undefined}
      >
        {isBareUrlLink(href, children) ? displayUrl(children) : children}
      </a>
    ),
  img: ({ alt }: { alt?: string }) => (
    <span role="img" aria-label={alt ?? undefined}>
      {alt}
    </span>
  ),
  input: ({ checked }: { checked?: boolean }) => (
    <input type="checkbox" checked={checked ?? false} disabled readOnly />
  ),
  table: ({ children }: { children?: ReactNode }) => (
    <div className="source-document-table-scroll">
      <table>{children}</table>
    </div>
  ),
};

export default function SourceDocumentContent({ content, title }: SourceDocumentContentProps) {
  if (!content.trim()) return null;

  return (
    <div className="source-document-body">
      <ReactMarkdown
        remarkPlugins={[remarkGfm, omitMatchingLeadingTitle(title)]}
        components={markdownComponents}
      >
        {content}
      </ReactMarkdown>
    </div>
  );
}
