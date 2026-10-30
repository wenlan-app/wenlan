// SPDX-License-Identifier: AGPL-3.0-only

/** What a `[[wikilink]]` reads as: a label, plus a link target when it resolves. */
export interface ResolvedReadingWikilink {
  label: string;
  href?: string;
}

interface MarkdownNode {
  type: string;
  value?: string;
  url?: string;
  children?: MarkdownNode[];
  position?: { start: { offset?: number }; end: { offset?: number } };
}

interface ReadingWikilinkOptions {
  /** The exact Markdown the tree was parsed from, to see escapes in text nodes. */
  source: string;
  /** Returns how a wikilink reads, or null to leave `[[...]]` as written. */
  resolve(inner: string): ResolvedReadingWikilink | null;
}

const WIKILINK = /\[\[([^\]\n]+)\]\]/g;
const LINK_NODES = new Set(["link", "linkReference", "image", "imageReference"]);

/**
 * A remark plugin that rewrites `[[wikilinks]]` after Markdown is parsed, so
 * only prose text changes: code spans and blocks, existing links, embeds
 * (`![[...]]`) and escaped brackets (`\[\[...]]`) stay exactly as written.
 */
export function remarkReadingWikilinks(options: ReadingWikilinkOptions) {
  const rewriteText = (node: MarkdownNode): MarkdownNode[] => {
    const value = node.value ?? "";
    const raw = options.source.slice(node.position?.start.offset, node.position?.end.offset);
    if (/\\\[/.test(raw)) return [node];

    const result: MarkdownNode[] = [];
    let cursor = 0;
    for (const match of value.matchAll(WIKILINK)) {
      if (match.index > 0 && value[match.index - 1] === "!") continue;
      const target = options.resolve(match[1]);
      if (!target) continue;
      if (match.index > cursor) result.push({ type: "text", value: value.slice(cursor, match.index) });
      result.push(
        target.href
          ? { type: "link", url: target.href, children: [{ type: "text", value: target.label }] }
          : { type: "text", value: target.label },
      );
      cursor = match.index + match[0].length;
    }
    if (cursor === 0) return [node];
    if (cursor < value.length) result.push({ type: "text", value: value.slice(cursor) });
    return result;
  };

  const visit = (node: MarkdownNode) => {
    if (!node.children || LINK_NODES.has(node.type)) return;
    node.children = node.children.flatMap((child) => {
      if (child.type === "text" && child.value) return rewriteText(child);
      visit(child);
      return [child];
    });
  };

  return (tree: unknown) => visit(tree as MarkdownNode);
}
