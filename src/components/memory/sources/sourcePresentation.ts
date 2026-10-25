// SPDX-License-Identifier: AGPL-3.0-only
import type { IndexedFileInfo } from "../../../lib/tauri";

export type SourceKind = "pdf" | "text" | "image" | "webpage" | "file";
/** Apps whose documents an agent can save with their permanent link. */
export type SourceApp =
  | "gmail" | "googleDrive" | "notion" | "slack" | "github"
  | "x" | "threads" | "instagram" | "xiaohongshu" | "youtube";
/** Brand names are not translated, except where the brand itself has a name per language. */
export const SOURCE_APP_NAMES: Readonly<Record<SourceApp, string>> = {
  gmail: "Gmail",
  googleDrive: "Google Drive",
  notion: "Notion",
  slack: "Slack",
  github: "GitHub",
  x: "X",
  threads: "Threads",
  instagram: "Instagram",
  xiaohongshu: "RedNote",
  youtube: "YouTube",
};
const LOCAL_APP_NAMES: Readonly<Partial<Record<SourceApp, Readonly<Record<string, string>>>>> = {
  xiaohongshu: { "zh-Hans": "小红书", "zh-Hant": "小紅書" },
};
/** The app's name as people using `language` know it. */
export function sourceAppName(app: SourceApp, language?: string): string {
  return (language && LOCAL_APP_NAMES[app]?.[language]) || SOURCE_APP_NAMES[app];
}
/** Every name an app goes by, for search. */
export function sourceAppSearchNames(app: SourceApp): string {
  return [SOURCE_APP_NAMES[app], ...Object.values(LOCAL_APP_NAMES[app] ?? {})].join(" ");
}
/** Note apps whose connected vault or graph Wenlan follows. */
export type NoteApp = "obsidian" | "logseq";
/** Whether a source type or `source_agent` names a note app. Their notes are
 *  stored in sections, `<source>::<path>::<n>`. */
export function isNoteApp(value: string | null | undefined): value is NoteApp {
  return value === "obsidian" || value === "logseq";
}
/** What to print for a stored source id such as `obsidian-vault::/Users/me/vault/Note.md::0`:
 *  the file name, never the internal id. Plain ids and links are returned as they are. */
export function sourceLocatorLabel(locator: string): string {
  const separator = locator.indexOf("::");
  if (separator < 0) return locator;
  const path = locator.slice(separator + 2).replace(/::\d+$/, "").replace(/\\/g, "/").replace(/\/+$/, "");
  return path.split("/").pop() || locator;
}
export function isSourceApp(value: string): value is SourceApp {
  return Object.prototype.hasOwnProperty.call(SOURCE_APP_NAMES, value);
}
export type SourcePresentationInput = Pick<IndexedFileInfo, "source" | "source_id" | "title" | "url" | "source_agent">;
const WEBPAGE_EXTENSIONS = new Set(["html", "htm", "php", "asp", "aspx"]);

export interface SourcePresentationLabels {
  document: string;
  webExcerpt: string;
  /** The app language, for app names that differ by language. */
  language?: string;
}

export interface SourcePresentation {
  kind: SourceKind;
  /** Set when a web link comes from a recognized app; its name replaces the generic format and host. */
  app: SourceApp | null;
  /** What to draw for this source: the app's icon when recognized, otherwise the kind's. */
  iconKind: SourceKind | SourceApp;
  format: string;
  origin: string | null;
  metadata: string;
}

/** Recognize the app behind an http(s) link from its host; null for any other site. */
export function getSourceAppFromUrl(value: string | URL | null | undefined): SourceApp | null {
  const url = typeof value === "string" ? httpUrl(value) : value ?? null;
  if (!url || (url.protocol !== "http:" && url.protocol !== "https:")) return null;
  const host = url.hostname.toLowerCase().replace(/\.$/, "");
  const under = (domain: string) => host === domain || host.endsWith(`.${domain}`);
  if (host === "mail.google.com") return "gmail";
  if (host === "docs.google.com" || host === "drive.google.com") return "googleDrive";
  if (host === "notion.so" || host === "www.notion.so" || host === "app.notion.com" || host.endsWith(".notion.site")) return "notion";
  if (under("slack.com")) return "slack";
  if (host === "github.com") return "github";
  if (under("x.com") || under("twitter.com")) return "x";
  if (under("threads.net") || under("threads.com")) return "threads";
  if (under("instagram.com")) return "instagram";
  if (under("xiaohongshu.com") || under("xhslink.com")) return "xiaohongshu";
  if (under("youtube.com") || host === "youtu.be" || under("youtube-nocookie.com")) return "youtube";
  return null;
}

const NOT_HANDLES: Partial<Record<SourceApp, ReadonlySet<string>>> = {
  x: new Set(["home", "explore", "search", "i", "settings", "hashtag", "intent", "share", "messages", "notifications", "compose", "login", "signup", "tos", "privacy"]),
  instagram: new Set(["p", "reel", "reels", "stories", "explore", "tv", "accounts", "direct", "about", "developer", "legal"]),
};

/** The account a social link belongs to, such as `simonw` for an X post, so
 *  saved links are told apart by who wrote them. */
export function sourceHandleFromUrl(value: string): string | null {
  const url = httpUrl(value);
  const app = getSourceAppFromUrl(url);
  if (!url || !app) return null;
  const first = url.pathname.split("/").filter(Boolean)[0] ?? "";
  let handle: string | null = null;
  if (app === "threads") handle = /^@([A-Za-z0-9._]{1,30})$/.exec(first)?.[1] ?? null;
  else if (app === "x") handle = /^[A-Za-z0-9_]{1,15}$/.test(first) ? first : null;
  else if (app === "instagram") handle = /^[A-Za-z0-9._]{1,30}$/.test(first) ? first : null;
  return handle && !NOT_HANDLES[app]?.has(handle.toLowerCase()) ? handle : null;
}

/** What a social or mail link points at, so a link saved without its page
 *  can be named "Instagram post" rather than only by its site. */
export function sourceLinkKind(value: string): "post" | "note" | "message" | "video" | null {
  const url = httpUrl(value);
  const app = getSourceAppFromUrl(url);
  if (!url || !app) return null;
  const parts = url.pathname.split("/").filter(Boolean);
  if (app === "x") return parts.includes("status") ? "post" : null;
  if (app === "threads") return parts.includes("post") ? "post" : null;
  if (app === "instagram") return ["p", "reel", "reels", "tv"].includes(parts[0] ?? "") ? "post" : null;
  if (app === "xiaohongshu") {
    return url.hostname.endsWith("xhslink.com") || ["explore", "discovery"].includes(parts[0] ?? "") ? "note" : null;
  }
  if (app === "youtube") {
    return url.hostname === "youtu.be" || ["watch", "shorts", "live"].includes(parts[0] ?? "") ? "video" : null;
  }
  if (app === "gmail" || app === "slack") return "message";
  return null;
}

function httpUrl(value: string | null | undefined): URL | null {
  if (!value) return null;
  try {
    const url = new URL(value);
    return url.protocol === "http:" || url.protocol === "https:" ? url : null;
  } catch {
    return null;
  }
}

function sourceUrl(file: SourcePresentationInput): URL | null {
  const idUrl = httpUrl(file.source_id);
  if (file.source === "webpage") return idUrl ?? httpUrl(file.url);
  if (idUrl) return idUrl;

  // A local path is stronger provenance than an optional URL attached to the
  // indexed record (for example, a source article URL stored with an imported PDF).
  const separator = file.source_id.indexOf("::");
  const rawId = separator >= 0 ? file.source_id.slice(separator + 2) : file.source_id;
  const hasPathShape = separator >= 0 || /[\\/]/.test(rawId) || /\.[a-z0-9]+$/i.test(rawId);
  return hasPathShape ? null : httpUrl(file.url);
}

function sourcePath(file: SourcePresentationInput, url: URL | null): { value: string | null; known: boolean } {
  const separator = file.source_id.indexOf("::");
  const rawId = separator >= 0 ? file.source_id.slice(separator + 2) : file.source_id;
  // Obsidian and Logseq IDs append a section ordinal after the real file path.
  // Keep that storage identifier intact; only strip it for visual classification.
  const raw = separator >= 0 && (file.source === "obsidian" || isNoteApp(file.source_agent))
    ? rawId.replace(/::\d+$/, "") : rawId;
  if (!raw || httpUrl(raw)) return { value: null, known: separator >= 0 };

  // Namespaced IDs are used for imported paths, while plain opaque IDs (such
  // as a database key) are not enough evidence to override a useful title.
  const hasPathShape = separator >= 0 || /[\\/]/.test(raw) || /\.[a-z0-9]+$/i.test(raw);
  if (hasPathShape) return { value: raw.replace(/\\/g, "/"), known: true };

  // A webpage URL is the location, never a local path.
  if (url) return { value: null, known: false };
  return { value: null, known: false };
}

function extensionOf(value: string | null): string | null {
  if (!value) return null;
  const name = value.replace(/\\/g, "/").split("/").pop() ?? "";
  return /\.([a-z0-9]+)$/i.exec(name)?.[1].toLowerCase() ?? null;
}

function parentFolder(path: string | null): string | null {
  if (!path) return null;
  const normalized = path.replace(/\\/g, "/").replace(/\/+$/, "");
  const split = normalized.lastIndexOf("/");
  return split > 0 ? normalized.slice(0, split).split("/").pop() || null : null;
}

function sourceFormat(extension: string | null): { kind: SourceKind; label: string } | null {
  if (!extension) return null;
  if (extension === "pdf") return { kind: "pdf", label: "PDF" };
  if (["md", "markdown", "mdx"].includes(extension)) return { kind: "text", label: "Markdown" };
  if (["txt", "text"].includes(extension)) return { kind: "text", label: extension === "txt" ? "TXT" : "Text" };
  if (["png", "jpg", "jpeg", "gif", "webp", "svg", "avif", "bmp", "tif", "tiff"].includes(extension)) {
    return { kind: "image", label: extension.toUpperCase() };
  }
  return null;
}

interface SourceClassification {
  kind: SourceKind;
  format: string | null;
  origin: string | null;
  app: SourceApp | null;
}

function classifySource(file: SourcePresentationInput): SourceClassification {
  const url = sourceUrl(file);
  const path = sourcePath(file, url);
  const pathExtension = extensionOf(path.value);
  const urlExtension = url ? extensionOf(url.pathname) : null;
  const titleExtension = !path.known && !url ? extensionOf(file.title) : null;
  const format = sourceFormat(pathExtension ?? urlExtension ?? titleExtension);
  const host = url?.hostname || null;

  if (format) {
    return { kind: format.kind, format: format.label, origin: host ?? parentFolder(path.value), app: null };
  }

  // A URL without a recognized file format represents a web page. This also
  // handles query strings and fragments because extension parsing uses URL.pathname.
  if (
    (url && (!urlExtension || WEBPAGE_EXTENSIONS.has(urlExtension))) ||
    (file.source === "webpage" && (!url || !urlExtension || WEBPAGE_EXTENSIONS.has(urlExtension)))
  ) {
    return { kind: "webpage", format: null, origin: host, app: getSourceAppFromUrl(url) };
  }

  return { kind: "file", format: null, origin: host ?? parentFolder(path.value), app: null };
}

/** Return only the visual resource kind, suitable for icon-only surfaces. */
export function getSourceKind(file: SourcePresentationInput): SourceKind {
  return classifySource(file).kind;
}

/** Resolve a source's content format and provenance consistently across views. */
export function getSourcePresentation(
  file: SourcePresentationInput,
  labels: SourcePresentationLabels,
): SourcePresentation {
  const classification = classifySource(file);
  if (classification.format) {
    return {
      kind: classification.kind,
      app: null,
      iconKind: classification.kind,
      format: classification.format,
      origin: classification.origin,
      metadata: [classification.format, classification.origin].filter(Boolean).join(" · "),
    };
  }

  if (classification.app) {
    // The app is the provenance: name it instead of the generic format and raw host.
    const name = sourceAppName(classification.app, labels.language);
    return { kind: classification.kind, app: classification.app, iconKind: classification.app, format: name, origin: null, metadata: name };
  }

  const format = classification.kind === "webpage" ? labels.webExcerpt : labels.document;
  return {
    ...classification,
    iconKind: classification.kind,
    format,
    metadata: [format, classification.origin].filter(Boolean).join(" · "),
  };
}
