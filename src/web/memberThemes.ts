// SPDX-License-Identifier: AGPL-3.0-only
/**
 * Custom themes for a team member: kept in this browser's localStorage, never
 * sent to a server. Answers the same shapes as the daemon's /api/themes routes
 * (`wenlan_core::theme`), minus images, which a browser cannot store here.
 */
import type {
  ActiveTheme, ThemeBase, ThemeCheck, ThemeDetail, ThemeFile, ThemeList, ThemeProblem, ThemeSummary,
} from "../lib/tauri";

const STORAGE_KEY = "wenlan-member-themes";
const SCHEMA = 1;
const MAX_NAME = 60;

const ALLOWED_FONTS = [
  "Fraunces", "Instrument Sans", "JetBrains Mono", "Georgia", "Times New Roman", "Palatino", "Helvetica Neue",
  "Arial", "Verdana", "Menlo", "Courier New", "PingFang TC", "PingFang SC", "Noto Sans CJK TC",
  "Noto Serif CJK TC", "serif", "sans-serif", "monospace", "system-ui",
];

// name, tokens, dark, light (mirrors SLOTS in wenlan-core theme/mod.rs)
export const SLOTS: [string, string[], string, string][] = [
  ["background", ["--mem-bg"], "#191C24", "#FCFCFB"],
  ["surface", ["--mem-surface", "--mem-detail-surface", "--mem-popover"], "#202430", "#FFFFFF"],
  ["card", ["--mem-card-surface"], "#20232A", "#F8F8F7"],
  ["sidebar", ["--mem-sidebar"], "#161920", "#FFFFFF"],
  ["text", ["--mem-text", "--mem-brand-text"], "#F1EFE8", "#1A1A2E"],
  ["textSecondary", ["--mem-text-secondary"], "#C9C8D1", "#586174"],
  ["textTertiary", ["--mem-text-tertiary", "--mem-text-meta-aa"], "#969BAD", "#687184"],
  ["border", ["--mem-border"], "#333A4B", "#E3E7EE"],
  ["accent", ["--mem-accent-indigo", "--mem-accent-page"], "#A9AEF2", "#5E58C8"],
  ["textOnAccent", ["--mem-text-on-accent"], "#11131A", "#FFFFFF"],
  ["accentWarm", ["--mem-accent-warm"], "#BB8967", "#B46A3A"],
  ["link", ["--mem-link"], "#9095D8", "#5E58C8"],
  ["danger", ["--mem-status-danger-text", "--mem-danger"], "#D98A78", "#A4442F"],
  ["warning", ["--mem-status-warning-text"], "#DCC36F", "#8C5F16"],
  ["success", ["--mem-status-success-text"], "#BBC7AE", "#4F7558"],
  ["graphProject", ["--kg-project"], "#E0AE57", "#C0851F"],
  ["graphTool", ["--kg-tool"], "#6DA9E7", "#2D78BD"],
  ["graphOrg", ["--kg-org"], "#E2805E", "#C95A37"],
  ["graphPerson", ["--kg-person"], "#B99BDE", "#8160B5"],
  ["graphConcept", ["--kg-concept"], "#7CBD89", "#4A925C"],
  ["graphMemory", ["--kg-memory"], "#D7C9AE", "#B9A987"],
  ["graphPage", ["--kg-page"], "#67A6A1", "#5E9C98"],
  ["graphEdge", ["--kg-edge"], "#444956", "#B9BDC5"],
];
const slot = (name: string) => SLOTS.find((s) => s[0] === name);

const DERIVED: [string, string, number, number][] = [
  ["--mem-indigo-bg", "accent", 0.13, 0.08],
  ["--mem-accent-indigo-border", "accent", 0.42, 0.34],
  ["--mem-focus-ring", "accent", 0.55, 0.45],
  ["--mem-distilled-border", "accent", 0.36, 0.28],
  ["--mem-distilled-bg", "accent", 0.09, 0.06],
  ["--mem-detail-divider", "text", 0.11, 0.1],
  ["--mem-hover", "textTertiary", 0.09, 0.08],
  ["--mem-hover-strong", "textTertiary", 0.15, 0.14],
];

const CONTRAST: [string, string, number][] = [
  ["text", "background", 4.5], ["text", "surface", 4.5], ["text", "card", 4.5], ["text", "sidebar", 4.5],
  ["textSecondary", "background", 4.5], ["textSecondary", "surface", 4.5], ["textTertiary", "background", 3],
  ["link", "background", 4.5], ["textOnAccent", "accent", 4.5], ["accent", "background", 3],
  ["danger", "background", 3],
];

type Store = { themes: Record<string, ThemeFile>; active: string | null };

function read(): Store {
  try {
    const parsed = JSON.parse(window.localStorage.getItem(STORAGE_KEY) ?? "null");
    if (parsed && typeof parsed === "object" && parsed.themes && typeof parsed.themes === "object") {
      return { themes: parsed.themes, active: typeof parsed.active === "string" ? parsed.active : null };
    }
  } catch {
    // absent, blocked or broken storage: start empty
  }
  return { themes: {}, active: null };
}

function write(store: Store): void {
  try {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(store));
  } catch {
    // The theme still applies for this session; it just will not be remembered.
  }
}

// Storage that cannot be written still has to behave within one session.
let session: Store | null = null;
function load(): Store {
  const stored = read();
  if (session && Object.keys(stored.themes).length === 0 && Object.keys(session.themes).length > 0) return session;
  return stored;
}
function commit(store: Store): void {
  session = store;
  write(store);
}

const problem = (field: string, message: string): ThemeProblem => ({ field, message });
const validId = (id: string) => /^[a-z0-9][a-z0-9-]{0,63}$/.test(id) && id !== "active" && id !== "check";

function hex6(value: unknown): [number, number, number] | null {
  if (typeof value !== "string" || !/^#[0-9a-fA-F]{6}$/.test(value)) return null;
  return [1, 3, 5].map((i) => parseInt(value.slice(i, i + 2), 16)) as [number, number, number];
}

function luminance([r, g, b]: [number, number, number]): number {
  const c = (v: number) => {
    const x = v / 255;
    return x <= 0.03928 ? x / 12.92 : ((x + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * c(r) + 0.7152 * c(g) + 0.0722 * c(b);
}

function effective(theme: ThemeFile, name: string): [number, number, number] | null {
  const s = slot(name);
  if (!s) return null;
  return hex6(theme.colors?.[name] ?? (theme.base === "dark" ? s[2] : s[3]));
}

export function checkMemberTheme(theme: ThemeFile): ThemeProblem[] {
  const problems: ThemeProblem[] = [];
  if (theme.schema !== SCHEMA) problems.push(problem("schema", `This Wenlan reads theme schema ${SCHEMA}.`));
  const name = (theme.name ?? "").trim();
  // eslint-disable-next-line no-control-regex
  if (!name || [...name].length > MAX_NAME || /[\u0000-\u001f\u007f]/.test(name)) {
    problems.push(problem("name", `Give the theme a name of 1 to ${MAX_NAME} characters.`));
  }
  let colorsOk = true;
  const colors = theme.colors ?? {};
  for (const [key, value] of Object.entries(colors)) {
    if (!slot(key)) {
      problems.push(problem(`colors.${key}`, `Unknown color slot. Use one of: ${SLOTS.map((s) => s[0]).join(", ")}.`));
      colorsOk = false;
    } else if (!hex6(value)) {
      problems.push(problem(`colors.${key}`, "Write the color as #RRGGBB, for example #E58BC0."));
      colorsOk = false;
    }
  }
  if (colorsOk) {
    for (const [fg, bg, min] of CONTRAST) {
      if (!(fg in colors) && !(bg in colors)) continue;
      const a = effective(theme, fg);
      const b = effective(theme, bg);
      if (!a || !b) continue;
      const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
      const ratio = (hi + 0.05) / (lo + 0.05);
      if (ratio + 1e-9 < min) {
        problems.push(problem(`colors.${fg}`, `${fg} on ${bg} has contrast ${ratio.toFixed(1)}:1; it needs at least ${min}:1 to stay readable.`));
      }
    }
  }
  for (const key of ["heading", "body", "mono"] as const) {
    const font = theme.fonts?.[key];
    if (font !== undefined && !ALLOWED_FONTS.includes(font)) {
      problems.push(problem(`fonts.${key}`, `Use one of these fonts: ${ALLOWED_FONTS.join(", ")}.`));
    }
  }
  const radius = theme.radius;
  if (radius !== undefined && !(Number.isFinite(radius) && radius >= 0 && radius <= 2.5)) {
    problems.push(problem("radius", "Use a corner roundness from 0 (square) to 2.5; 1 is Wenlan's default."));
  }
  for (const [key, value] of Object.entries(theme.advanced ?? {})) {
    if (!/^--[a-z0-9-]+$/.test(key)) {
      problems.push(problem(`advanced.${key}`, "Wenlan has no token with this name."));
    } else if (!/^#([0-9a-fA-F]{6}|[0-9a-fA-F]{8})$/.test(String(value))) {
      problems.push(problem(`advanced.${key}`, "Write the value as #RRGGBB or #RRGGBBAA."));
    } else if (key.startsWith("--kg-") && !hex6(value)) {
      problems.push(problem(`advanced.${key}`, "Map colors must be opaque: write #RRGGBB."));
    }
  }
  // A member's browser keeps no image files, so a theme that names one is refused.
  if (theme.images?.background) problems.push(problem("images.background.file", "Images are not available in a browser."));
  if (theme.images?.sticker) problems.push(problem("images.sticker.file", "Images are not available in a browser."));
  return problems;
}

function fontStack(font: string, fallback: string): string {
  return `${font.includes(" ") ? `'${font}'` : font}, ${fallback}`;
}

export function memberThemeTokens(theme: ThemeFile): Record<string, string> {
  const tokens: Record<string, string> = {};
  const dark = theme.base === "dark";
  for (const [name, list] of SLOTS.map((s) => [s[0], s[1]] as const)) {
    const value = theme.colors?.[name];
    if (value) for (const token of list) tokens[token] = value.toUpperCase();
  }
  for (const [token, slotName, darkAlpha, lightAlpha] of DERIVED) {
    const rgb = hex6(theme.colors?.[slotName]);
    if (rgb) tokens[token] = `rgba(${rgb[0]}, ${rgb[1]}, ${rgb[2]}, ${dark ? darkAlpha : lightAlpha})`;
  }
  if (theme.fonts?.heading) tokens["--mem-font-heading"] = fontStack(theme.fonts.heading, "Georgia, serif");
  if (theme.fonts?.body) tokens["--mem-font-body"] = fontStack(theme.fonts.body, "-apple-system, sans-serif");
  if (theme.fonts?.mono) tokens["--mem-font-mono"] = fontStack(theme.fonts.mono, "monospace");
  if (theme.radius !== undefined) tokens["--mem-radius-scale"] = String(Math.round(theme.radius * 100) / 100);
  for (const [token, value] of Object.entries(theme.advanced ?? {})) tokens[token] = String(value).toUpperCase();
  return tokens;
}

function template(name: string, base: ThemeBase): ThemeFile {
  return {
    schema: SCHEMA,
    name,
    base,
    colors: Object.fromEntries(SLOTS.map((s) => [s[0], base === "dark" ? s[2] : s[3]])),
    fonts: {},
    radius: 1,
    images: {},
    advanced: {},
  };
}

function swatch(theme: ThemeFile): Record<string, string> {
  const out: Record<string, string> = {};
  for (const name of ["background", "sidebar", "text", "accent"]) {
    const rgb = effective(theme, name);
    if (rgb) out[name] = `#${rgb.map((v) => v.toString(16).padStart(2, "0")).join("").toUpperCase()}`;
  }
  return out;
}

function newId(store: Store, name: string): string {
  const slug = name.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 40).replace(/-+$/, "");
  const base = slug && validId(slug) ? slug : "theme";
  let id = base;
  for (let n = 2; id in store.themes; n++) id = `${base}-${n}`;
  return id;
}

function revision(id: string, theme: ThemeFile): string {
  const text = id + JSON.stringify(theme);
  let h = 5381;
  for (let i = 0; i < text.length; i++) h = ((h * 33) ^ text.charCodeAt(i)) >>> 0;
  return h.toString(16).padStart(8, "0");
}

type Write<T> = { status: "ok"; value: T } | { status: "refused"; problems: ThemeProblem[] };
const refused = (problems: ThemeProblem[]): Write<never> => ({ status: "refused", problems });

export function listMemberThemes(): ThemeList {
  const store = load();
  const themes: ThemeSummary[] = Object.entries(store.themes)
    .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
    .map(([id, theme]) => ({
      id, name: theme.name, base: theme.base, author: theme.author,
      problems: checkMemberTheme(theme), swatch: swatch(theme),
    }));
  return {
    themes_dir: "",
    ...(store.active && store.active in store.themes ? { active: store.active } : {}),
    themes,
    allowed_fonts: ALLOWED_FONTS,
  };
}

export function getMemberActiveTheme(): ActiveTheme {
  const store = load();
  const id = store.active;
  const theme = id ? store.themes[id] : undefined;
  if (!id || !theme) return { revision: "builtin", tokens: {}, problems: [] };
  const problems = checkMemberTheme(theme);
  if (problems.length > 0) return { id, revision: revision(id, theme), tokens: {}, problems };
  return { id, name: theme.name, base: theme.base, revision: revision(id, theme), tokens: memberThemeTokens(theme), problems };
}

export function getMemberTheme(id: string): ThemeDetail {
  const theme = load().themes[id];
  if (!theme) throw new Error("theme_not_found");
  return { id, theme, problems: checkMemberTheme(theme) };
}

export function createMemberTheme(name: string, base: ThemeBase): Write<ThemeDetail> {
  const store = load();
  const clean = String(name).trim();
  const id = newId(store, clean);
  const theme = template(clean, base);
  const problems = checkMemberTheme(theme);
  if (problems.length > 0) return refused(problems);
  commit({ ...store, themes: { ...store.themes, [id]: theme } });
  return { status: "ok", value: { id, theme, problems: [] } };
}

export function saveMemberTheme(id: string, theme: ThemeFile): Write<ThemeDetail> {
  if (!validId(id)) return refused([problem("id", "A theme id uses lowercase letters, digits and dashes, like night-bunny.")]);
  const problems = checkMemberTheme(theme);
  if (problems.length > 0) return refused(problems);
  const store = load();
  commit({ ...store, themes: { ...store.themes, [id]: theme } });
  return { status: "ok", value: { id, theme, problems: [] } };
}

export function checkMemberDraft(id: string, theme: ThemeFile): ThemeCheck {
  const problems = validId(id) ? checkMemberTheme(theme) : [problem("id", "A theme id uses lowercase letters, digits and dashes.")];
  return { id, ok: problems.length === 0, problems, ...(problems.length === 0 ? { tokens: memberThemeTokens(theme) } : {}) };
}

export function setMemberActiveTheme(id: string | null): Write<ActiveTheme> {
  const store = load();
  if (id !== null) {
    const theme = store.themes[id];
    const problems = theme ? checkMemberTheme(theme) : [problem("id", "No theme has this id.")];
    if (problems.length > 0) return refused(problems);
  }
  commit({ ...store, active: id });
  return { status: "ok", value: getMemberActiveTheme() };
}

/** Test hook: forget what this session kept in memory. */
export function resetMemberThemesForTests(): void {
  session = null;
}

/** The theme commands a member's browser answers itself, with no server call. */
export const MEMBER_THEME_COMMANDS: Record<string, (a: any) => unknown> = {
  list_themes: () => listMemberThemes(),
  get_active_theme: () => getMemberActiveTheme(),
  get_theme: (a) => getMemberTheme(String(a.id)),
  create_theme: (a) => createMemberTheme(a.name, a.base),
  save_theme: (a) => saveMemberTheme(String(a.id), a.theme),
  check_theme_draft: (a) => checkMemberDraft(String(a.id), a.theme),
  set_active_theme: (a) => setMemberActiveTheme(a.id ?? null),
  active_theme_image: () => null,
  theme_image: () => null,
};
