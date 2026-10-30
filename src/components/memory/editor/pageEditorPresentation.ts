// SPDX-License-Identifier: AGPL-3.0-only

function normalizeTitle(value: string): string {
  return value.trim().replace(/\s+/gu, " ");
}

/**
 * Presentation-only check for the common `Page.title` + leading `# title`
 * duplication. This never parses, strips, or rewrites the canonical source.
 */
export function leadingMarkdownH1MatchesTitle(
  source: string,
  title: string,
): boolean {
  const normalizedTitle = normalizeTitle(title);
  if (!normalizedTitle) return false;

  const withoutBom = source.startsWith("\uFEFF") ? source.slice(1) : source;
  const match =
    /^(?:[ \t]*(?:\r\n|\n|\r))*[ \t]{0,3}#[ \t]+([^\r\n]*?)(?=\r\n|\n|\r|$)/u.exec(
      withoutBom,
    );
  if (!match) return false;

  const heading = match[1].replace(/[ \t]+#+[ \t]*$/u, "");
  const normalizedHeading = normalizeTitle(heading);
  return Boolean(normalizedHeading) && normalizedHeading === normalizedTitle;
}

/**
 * Reading view of a body without its first H1 when that heading repeats the
 * page title (the page heading already shows it). Any other body, including
 * one whose first H1 differs from the title, comes back unchanged.
 */
export function withoutLeadingTitleH1(source: string, title: string): string {
  if (!leadingMarkdownH1MatchesTitle(source, title)) return source;
  const withoutBom = source.startsWith("﻿") ? source.slice(1) : source;
  return withoutBom.replace(
    /^(?:[ \t]*(?:\r\n|\n|\r))*[ \t]{0,3}#[ \t]+[^\r\n]*(?:\r\n|\n|\r|$)/u,
    "",
  );
}
