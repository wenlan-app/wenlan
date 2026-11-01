// SPDX-License-Identifier: AGPL-3.0-only
import { MEDIA_EMBED, parseMediaEmbed } from "./pageAttachments";

/**
 * The two Obsidian forms the reading view draws (Qi-Xuan, 2026-10-31: basic
 * Obsidian compatibility): `![[image.png]]` embeds show as the image, and
 * `> [!note]` callouts show as a box with their type. Both run after Markdown
 * is parsed, so code spans and blocks stay exactly as written. Everything
 * else Obsidian adds is left as text.
 */
interface MarkdownNode {
  type: string;
  value?: string;
  url?: string;
  alt?: string;
  children?: MarkdownNode[];
  data?: { hName?: string; hProperties?: Record<string, unknown> };
}

const SKIP = new Set(["link", "linkReference", "image", "imageReference", "code", "inlineCode", "html"]);

/** Turns `![[name.png|300]]` in prose into an image the renderer resolves. */
export function remarkObsidianEmbeds() {
  const rewrite = (node: MarkdownNode): MarkdownNode[] => {
    const value = node.value ?? "";
    const out: MarkdownNode[] = [];
    let cursor = 0;
    for (const match of value.matchAll(MEDIA_EMBED)) {
      const embed = parseMediaEmbed(match[1]);
      if (!embed) continue;
      if (match.index > cursor) out.push({ type: "text", value: value.slice(cursor, match.index) });
      out.push({
        type: "image",
        url: "",
        alt: embed.alt,
        data: { hProperties: { dataEmbed: match[1], ...(embed.width ? { width: embed.width } : {}) } },
      });
      cursor = match.index + match[0].length;
    }
    if (cursor === 0) return [node];
    if (cursor < value.length) out.push({ type: "text", value: value.slice(cursor) });
    return out;
  };
  const visit = (node: MarkdownNode) => {
    if (!node.children || SKIP.has(node.type)) return;
    node.children = node.children.flatMap((child) => {
      if (child.type === "text" && child.value) return rewrite(child);
      visit(child);
      return [child];
    });
  };
  return (tree: unknown) => visit(tree as MarkdownNode);
}

const CALLOUT = /^\[!([A-Za-z0-9_-]+)\][+-]?[ \t]*/;

/** The badge label for a callout type, as the editor shows it. */
export function calloutLabel(type: string): string {
  return type.split(/[-_]+/).filter(Boolean).map((part) => part[0].toUpperCase() + part.slice(1)).join(" ");
}

/** Marks `> [!type] Title` blockquotes as callout boxes with a type badge. */
export function remarkCallouts() {
  const visit = (node: MarkdownNode) => {
    for (const child of node.children ?? []) {
      if (child.type === "blockquote") markCallout(child);
      visit(child);
    }
  };
  const markCallout = (quote: MarkdownNode) => {
    const paragraph = quote.children?.[0];
    const first = paragraph?.type === "paragraph" ? paragraph.children?.[0] : undefined;
    if (!paragraph || first?.type !== "text") return;
    const match = CALLOUT.exec(first.value ?? "");
    if (!match) return;
    const type = match[1].toLowerCase();
    const rest = (first.value ?? "").slice(match[0].length);
    const newline = rest.indexOf("\n");
    const title = newline < 0 ? rest : rest.slice(0, newline);
    const bodyStart = newline < 0 ? "" : rest.slice(newline + 1);
    const others = (paragraph.children ?? []).slice(1);
    // The first line is the box's heading: its type, then any title.
    const heading: MarkdownNode = {
      type: "paragraph",
      data: { hProperties: { className: ["page-callout-title"] } },
      children: [
        {
          type: "emphasis",
          data: { hName: "span", hProperties: { className: ["page-callout-badge"] } },
          children: [{ type: "text", value: calloutLabel(type) }],
        },
        ...(title.trim() ? [{ type: "text", value: title }] : []),
        ...(newline < 0 ? others : []),
      ],
    };
    const body: MarkdownNode[] = newline < 0
      ? []
      : [{ type: "paragraph", children: [...(bodyStart ? [{ type: "text", value: bodyStart }] : []), ...others] }];
    quote.children = [heading, ...body, ...(quote.children ?? []).slice(1)];
    quote.data = {
      ...quote.data,
      hProperties: { ...quote.data?.hProperties, className: ["page-callout", `page-callout-${type}`], dataCallout: type },
    };
  };
  return (tree: unknown) => visit(tree as MarkdownNode);
}
