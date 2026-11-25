// SPDX-License-Identifier: AGPL-3.0-only

/**
 * A sentence of a page's Markdown as a person reads it, for a place that
 * shows it as plain text (what a sentence said before an AI update, or one it
 * removed): links reduced to their words, citation markers dropped, and code,
 * emphasis, list and heading marks taken off. Underscores inside words stay,
 * so `snake_case` names read as written.
 */
export function readableSentence(markdown: string): string {
  return markdown
    // Citation markers, bare or already turned into links.
    .replace(/ *(?:\[\d+\](?:\(#citation:\d+\))?)+(?=[.,;:!?])/g, "")
    .replace(/\[\d+\](?:\(#citation:\d+\))?/g, "")
    // [[Page]], [[Page#Heading]], [[Page|words]]
    .replace(/\[\[([^\]|#]+)(?:#[^\]|]*)?(?:\|([^\]]+))?\]\]/g, (_, target: string, label?: string) => (label ?? target).trim())
    // [words](url) and ![alt](url)
    .replace(/!?\[([^\]]*)\]\([^)]*\)/g, "$1")
    // `code` (any run of backticks)
    .replace(/(`+)([^`]|[^`][\s\S]*?[^`])\1(?!`)/g, (_, _ticks: string, code: string) => code.trim())
    // **strong**, __strong__, *em*, _em_, ~~struck~~ around words only
    .replace(/(^|[^\w*~])(\*\*|__|~~)(?=\S)([\s\S]*?\S)\2(?![\w*~])/g, "$1$3")
    .replace(/(^|[^\w*])([*_])(?=\S)([\s\S]*?\S)\2(?![\w*])/g, "$1$3")
    // A list item, task box, heading or quote mark at the start.
    .replace(/^\s*(?:>\s*)*(?:#{1,6}\s+|(?:[-*+]|\d+[.)])\s+(?:\[[ xX]\]\s+)?)?/, "")
    .replace(/\s+/g, " ")
    .trim();
}
