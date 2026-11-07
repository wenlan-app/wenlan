// SPDX-License-Identifier: AGPL-3.0-only
export interface ZoteroAnnotationTarget {
  attachmentKey: string;
  page: number;
  annotationKey: string;
}

/** Accept only Zotero's canonical annotation-reading link, without URL
 * normalization that could disguise credentials, escapes, or extra actions. */
export function parseZoteroAnnotationLink(href: string | undefined): ZoteroAnnotationTarget | null {
  if (!href) return null;
  const match = /^zotero:\/\/open-pdf\/library\/items\/([A-Z0-9]{8})\?((?:page=[1-9]\d*&annotation=[A-Z0-9]{8})|(?:annotation=[A-Z0-9]{8}&page=[1-9]\d*))$/.exec(href);
  if (!match || match[0] !== href) return null;
  const params = new URLSearchParams(match[2]);
  const page = Number(params.get("page"));
  if (!Number.isSafeInteger(page)) return null;
  return { attachmentKey: match[1], page, annotationKey: params.get("annotation")! };
}
