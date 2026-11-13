// SPDX-License-Identifier: AGPL-3.0-only
/**
 * Settings > Appearance: pick a theme, edit a custom one with a live preview,
 * or ask an AI tool to make one (sketch A, custom-themes plan v2).
 *
 * Every rule lives in the daemon (`wenlan_core::theme`): the editor sends the
 * unsaved theme to `check_theme_draft` and shows the problems and preview
 * tokens the daemon returns, and a save or switch the daemon refuses comes
 * back as its problems.
 */
import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { useTranslation } from "react-i18next";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { toast } from "sonner";
import { useTheme, type Theme } from "../../../../lib/theme";
import { refreshCustomTheme } from "../../../../lib/customTheme";
import {
  checkThemeDraft,
  clipboardWrite,
  createTheme,
  getThemeDetail,
  getThemeImage,
  importThemeImage,
  listThemes,
  openThemesFolder,
  saveTheme,
  setActiveTheme,
  type ThemeFile,
  type ThemeProblem,
  type ThemeStickerCorner,
  type ThemeSummary,
} from "../../../../lib/tauri";
import { Button, SegmentedControl, Select } from "../primitives";
import HomeLookSetting from "./HomeLookSetting";
import "./appearance.css";

const EDITOR_SLOTS = ["background", "surface", "text", "accent", "accentWarm", "border"] as const;
const HEX = /^#[0-9A-Fa-f]{6}$/;
const RADIUS_BASE_PX = 8;
const IMAGE_LIMITS = { background: "5 MB", sticker: "1 MB" } as const;

type ImageKind = "background" | "sticker";

function sameTheme(a: ThemeFile | null, b: ThemeFile | null): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

function useThemeImage(id: string | null, file: string | undefined, kind: ImageKind, nonce: number): string | null {
  const [url, setUrl] = useState<string | null>(null);
  useEffect(() => {
    let cancelled = false;
    if (!id || !file) {
      setUrl(null);
      return;
    }
    getThemeImage(id, file, kind)
      .then((value) => !cancelled && setUrl(value))
      .catch(() => !cancelled && setUrl(null));
    return () => {
      cancelled = true;
    };
  }, [id, file, kind, nonce]);
  return url;
}

export default function AppearanceSection() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [builtInTheme, setBuiltInTheme] = useTheme();
  const themesQuery = useQuery({ queryKey: ["themes"], queryFn: listThemes });
  const list = themesQuery.data;
  const activeId = list?.active ?? null;

  const [editingId, setEditingId] = useState<string | null>(null);
  const [saved, setSaved] = useState<ThemeFile | null>(null);
  const [draft, setDraft] = useState<ThemeFile | null>(null);
  const [problems, setProblems] = useState<ThemeProblem[]>([]);
  const [previewTokens, setPreviewTokens] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState<"save" | "apply" | null>(null);
  const [imageNonce, setImageNonce] = useState(0);
  const [imageError, setImageError] = useState<string | null>(null);

  // Open the theme in use for editing, or the first custom one.
  useEffect(() => {
    if (editingId || !list) return;
    const first = list.active ?? list.themes.find((theme) => theme.problems.length === 0)?.id ?? null;
    if (first) setEditingId(first);
  }, [editingId, list]);

  useEffect(() => {
    let cancelled = false;
    setSaved(null);
    setDraft(null);
    setProblems([]);
    setPreviewTokens({});
    if (!editingId) return;
    getThemeDetail(editingId)
      .then((detail) => {
        if (cancelled) return;
        setSaved(detail.theme);
        setDraft(detail.theme);
        setProblems(detail.problems);
      })
      .catch(() => !cancelled && setEditingId(null));
    return () => {
      cancelled = true;
    };
  }, [editingId]);

  // Ask the daemon about the unsaved theme as it changes.
  const checkSeq = useRef(0);
  useEffect(() => {
    if (!editingId || !draft) return;
    const seq = ++checkSeq.current;
    const timer = window.setTimeout(() => {
      checkThemeDraft(editingId, draft)
        .then((check) => {
          if (seq !== checkSeq.current) return;
          setProblems(check.problems);
          if (check.ok) setPreviewTokens(check.tokens ?? {});
        })
        .catch(() => {});
    }, 150);
    return () => window.clearTimeout(timer);
  }, [editingId, draft]);

  const dirty = !sameTheme(saved, draft);
  const reloadList = useCallback(() => queryClient.invalidateQueries({ queryKey: ["themes"] }), [queryClient]);

  const update = (change: (theme: ThemeFile) => ThemeFile) => setDraft((current) => (current ? change(current) : current));

  const chooseBuiltIn = async (choice: Theme) => {
    setBuiltInTheme(choice);
    if (activeId) {
      const outcome = await setActiveTheme(null).catch(() => null);
      if (outcome?.status === "ok") await refreshCustomTheme(outcome.value);
      void reloadList();
    }
  };

  const save = async (): Promise<boolean> => {
    if (!editingId || !draft) return false;
    const outcome = await saveTheme(editingId, draft).catch(() => null);
    if (!outcome) {
      toast.error(t("appearance.saveFailed"));
      return false;
    }
    if (outcome.status === "refused") {
      setProblems(outcome.problems);
      return false;
    }
    setSaved(outcome.value.theme);
    setDraft(outcome.value.theme);
    void reloadList();
    return true;
  };

  const onSave = async () => {
    setBusy("save");
    try {
      if (await save()) {
        toast.success(t("appearance.saved"));
        if (activeId === editingId) await refreshCustomTheme();
      }
    } finally {
      setBusy(null);
    }
  };

  const onApply = async () => {
    if (!editingId) return;
    setBusy("apply");
    try {
      if (dirty && !(await save())) return;
      const outcome = await setActiveTheme(editingId).catch(() => null);
      if (!outcome) {
        toast.error(t("appearance.saveFailed"));
        return;
      }
      if (outcome.status === "refused") {
        setProblems(outcome.problems);
        return;
      }
      await refreshCustomTheme(outcome.value);
      void reloadList();
    } finally {
      setBusy(null);
    }
  };

  const onNewTheme = async () => {
    const base = document.documentElement.getAttribute("data-theme") === "light" ? "light" : "dark";
    const outcome = await createTheme(t("appearance.newThemeName"), base).catch(() => null);
    if (outcome?.status !== "ok") {
      toast.error(t("appearance.saveFailed"));
      return;
    }
    await reloadList();
    setEditingId(outcome.value.id);
  };

  const onAskAi = async () => {
    try {
      await clipboardWrite(t("appearance.askAiPrompt"));
      toast.success(t("appearance.askAiCopied"));
    } catch {
      toast.error(t("appearance.saveFailed"));
    }
  };

  const pickImage = async (kind: ImageKind) => {
    if (!editingId) return;
    setImageError(null);
    const picked = await openDialog({
      multiple: false,
      directory: false,
      filters: [{ name: "Images", extensions: ["png", "jpg", "jpeg", "webp"] }],
    }).catch(() => null);
    if (typeof picked !== "string") return;
    try {
      const file = await importThemeImage(editingId, kind, picked);
      setImageNonce((n) => n + 1);
      update((theme) => ({
        ...theme,
        images: {
          ...theme.images,
          [kind]:
            kind === "background"
              ? { placement: "sidebar", opacity: 0.35, ...theme.images.background, file }
              : { corner: "bottom-right", size: 88, ...theme.images.sticker, file },
        },
      }));
    } catch (error) {
      const message = String(error);
      setImageError(
        message.includes("too_large")
          ? t("appearance.imageTooLarge", { size: IMAGE_LIMITS[kind] })
          : message.includes("not_an_image")
            ? t("appearance.notAnImage")
            : t("appearance.imageFailed"),
      );
    }
  };

  const removeImage = (kind: ImageKind) =>
    update((theme) => {
      const images = { ...theme.images };
      delete images[kind];
      return { ...theme, images };
    });

  const backgroundUrl = useThemeImage(editingId, draft?.images.background?.file, "background", imageNonce);
  const stickerUrl = useThemeImage(editingId, draft?.images.sticker?.file, "sticker", imageNonce);

  const builtInCards: { value: Theme; label: string }[] = [
    { value: "system", label: t("appearance.system") },
    { value: "light", label: t("appearance.light") },
    { value: "dark", label: t("appearance.dark") },
  ];

  const fonts = list?.allowed_fonts ?? [];
  const editingSummary = list?.themes.find((theme) => theme.id === editingId);

  return (
    <div className="appearance mem-fade-up">
      <div className="appearance-heading">
        <h2 className="appearance-label">{t("appearance.themes")}</h2>
        <div className="appearance-actions">
          <Button variant="ghost" size="sm" onClick={() => void onAskAi()}>
            <SparkleIcon />
            {t("appearance.askAi")}
          </Button>
          <Button variant="ghost" size="sm" onClick={() => void openThemesFolder().catch(() => {})}>
            <FolderIcon />
            {t("appearance.openFolder")}
          </Button>
        </div>
      </div>

      {themesQuery.isError && <p className="appearance-error" role="alert">{t("appearance.loadFailed")}</p>}

      <div className="appearance-cards" role="list">
        {builtInCards.map((card) => {
          const inUse = !activeId && builtInTheme === card.value;
          return (
            <button
              key={card.value}
              type="button"
              role="listitem"
              className="appearance-card"
              data-in-use={inUse || undefined}
              aria-pressed={inUse}
              onClick={() => void chooseBuiltIn(card.value)}
            >
              <BuiltInSwatch kind={card.value} />
              <span className="appearance-card-name">
                {card.label}
                {inUse && <span className="appearance-chip">{t("appearance.inUse")}</span>}
              </span>
              <span className="appearance-card-meta">{t("appearance.builtIn")}</span>
            </button>
          );
        })}
        {list?.themes.map((theme) => (
          <CustomCard
            key={theme.id}
            theme={theme}
            inUse={activeId === theme.id}
            selected={editingId === theme.id}
            onSelect={() => setEditingId(theme.id)}
          />
        ))}
        <button type="button" role="listitem" className="appearance-card appearance-card-new" onClick={() => void onNewTheme()}>
          <span aria-hidden="true" className="appearance-card-plus">+</span>
          <span className="appearance-card-name">{t("appearance.newTheme")}</span>
        </button>
      </div>

      <HomeLookSetting />

      {editingId && draft && (
        <div className="appearance-editor">
          <div className="appearance-form">
            <div className="appearance-editor-title">
              <h3>{t("appearance.editing", { name: draft.name })}</h3>
              {list && <span className="appearance-path" title={`${list.themes_dir}/${editingId}/theme.json`}>{`${editingId}/theme.json`}</span>}
            </div>

            <Row label={t("appearance.name")}>
              <input
                className="appearance-text"
                value={draft.name}
                maxLength={60}
                onChange={(event) => update((theme) => ({ ...theme, name: event.currentTarget.value }))}
              />
            </Row>

            <Row label={t("appearance.colors")} top>
              <div className="appearance-swatches">
                {EDITOR_SLOTS.map((slot) => (
                  <ColorField
                    key={slot}
                    label={t(`appearance.slots.${slot}`)}
                    value={draft.colors[slot] ?? ""}
                    onChange={(value) => update((theme) => ({ ...theme, colors: { ...theme.colors, [slot]: value } }))}
                  />
                ))}
              </div>
            </Row>

            <Row label={t("appearance.headingFont")}>
              <FontSelect
                label={t("appearance.headingFont")}
                fonts={fonts}
                value={draft.fonts.heading}
                placeholder={t("appearance.defaultFont")}
                onChange={(heading) => update((theme) => ({ ...theme, fonts: { ...theme.fonts, heading } }))}
              />
            </Row>
            <Row label={t("appearance.bodyFont")}>
              <FontSelect
                label={t("appearance.bodyFont")}
                fonts={fonts}
                value={draft.fonts.body}
                placeholder={t("appearance.defaultFont")}
                onChange={(body) => update((theme) => ({ ...theme, fonts: { ...theme.fonts, body } }))}
              />
            </Row>

            <Row label={t("appearance.corners")}>
              <Slider
                label={t("appearance.corners")}
                min={0}
                max={2.5}
                step={0.25}
                value={draft.radius ?? 1}
                display={`${Math.round((draft.radius ?? 1) * RADIUS_BASE_PX)} px`}
                onChange={(radius) => update((theme) => ({ ...theme, radius }))}
              />
            </Row>

            <Row label={t("appearance.backgroundImage")}>
              <ImageField
                url={backgroundUrl}
                file={draft.images.background?.file}
                chooseLabel={draft.images.background ? t("appearance.replace") : t("appearance.choose")}
                removeLabel={t("appearance.remove")}
                onChoose={() => void pickImage("background")}
                onRemove={draft.images.background ? () => removeImage("background") : undefined}
              >
              </ImageField>
            </Row>
            {draft.images.background && (
              <Row label={t("appearance.placement")}>
                <SegmentedControl
                  aria-label={t("appearance.backgroundImage")}
                  value={draft.images.background.placement}
                  options={[
                    { value: "sidebar", label: t("appearance.placementSidebar") },
                    { value: "window", label: t("appearance.placementWindow") },
                  ]}
                  onChange={(placement) =>
                    update((theme) => ({
                      ...theme,
                      images: { ...theme.images, background: { ...theme.images.background!, placement } },
                    }))
                  }
                />
              </Row>
            )}
            {draft.images.background && (
              <Row label={t("appearance.opacity")}>
                <Slider
                  label={t("appearance.opacity")}
                  min={0}
                  max={1}
                  step={0.05}
                  value={draft.images.background.opacity}
                  display={`${Math.round(draft.images.background.opacity * 100)}%`}
                  onChange={(opacity) =>
                    update((theme) => ({
                      ...theme,
                      images: { ...theme.images, background: { ...theme.images.background!, opacity } },
                    }))
                  }
                />
              </Row>
            )}

            <Row label={t("appearance.sticker")}>
              <ImageField
                url={stickerUrl}
                file={draft.images.sticker?.file}
                chooseLabel={draft.images.sticker ? t("appearance.replace") : t("appearance.choose")}
                removeLabel={t("appearance.remove")}
                onChoose={() => void pickImage("sticker")}
                onRemove={draft.images.sticker ? () => removeImage("sticker") : undefined}
              >
              </ImageField>
            </Row>
            {draft.images.sticker && (
              <Row label={t("appearance.corner")}>
                <SegmentedControl<ThemeStickerCorner>
                  aria-label={t("appearance.sticker")}
                  value={draft.images.sticker.corner}
                  options={[
                    { value: "bottom-left", label: t("appearance.cornerBottomLeft") },
                    { value: "bottom-right", label: t("appearance.cornerBottomRight") },
                    { value: "top-right", label: t("appearance.cornerTopRight") },
                  ]}
                  onChange={(corner) =>
                    update((theme) => ({
                      ...theme,
                      images: { ...theme.images, sticker: { ...theme.images.sticker!, corner } },
                    }))
                  }
                />
              </Row>
            )}
            {draft.images.sticker && (
              <Row label={t("appearance.size")}>
                <Slider
                  label={t("appearance.size")}
                  min={48}
                  max={160}
                  step={4}
                  value={draft.images.sticker.size}
                  display={`${draft.images.sticker.size} px`}
                  onChange={(size) =>
                    update((theme) => ({
                      ...theme,
                      images: { ...theme.images, sticker: { ...theme.images.sticker!, size } },
                    }))
                  }
                />
              </Row>
            )}
            {imageError && <p className="appearance-error" role="alert">{imageError}</p>}

            <div className="appearance-footer">
              <Button variant="primary" size="md" loading={busy === "apply"} disabled={busy !== null || problems.length > 0} onClick={() => void onApply()}>
                {t("appearance.apply")}
              </Button>
              <Button size="md" loading={busy === "save"} disabled={busy !== null || !dirty || problems.length > 0} onClick={() => void onSave()}>
                {t("appearance.save")}
              </Button>
              <Button variant="ghost" size="md" disabled={!dirty || busy !== null} onClick={() => setDraft(saved)}>
                {t("appearance.revert")}
              </Button>
              <span className="appearance-status" data-ok={problems.length === 0 || undefined} role="status">
                {problems.length === 0 ? t("appearance.checksPass") : t("appearance.problems", { count: problems.length })}
              </span>
            </div>
            {problems.length > 0 && (
              <ul className="appearance-problems">
                {problems.map((problem) => (
                  <li key={`${problem.field}:${problem.message}`}>{problem.message}</li>
                ))}
              </ul>
            )}
            {editingSummary && editingSummary.problems.length > 0 && !dirty && problems.length === 0 && (
              <p className="appearance-error">{t("appearance.needsFixing")}</p>
            )}
          </div>

          <figure className="appearance-preview-wrap">
            <ThemePreview
              tokens={previewTokens}
              background={draft.images.background && backgroundUrl ? { url: backgroundUrl, ...draft.images.background } : null}
              sticker={draft.images.sticker && stickerUrl ? { url: stickerUrl, ...draft.images.sticker } : null}
            />
            <figcaption>
              <span>{t("appearance.preview")}</span>
              <span>{t("appearance.previewHint")}</span>
            </figcaption>
          </figure>
        </div>
      )}

    </div>
  );
}

function Row({ label, top, children }: { label: string; top?: boolean; children: React.ReactNode }) {
  return (
    <div className="appearance-row" data-top={top || undefined}>
      <span className="appearance-row-label">{label}</span>
      <div className="appearance-row-control">{children}</div>
    </div>
  );
}

function ColorField({ label, value, onChange }: { label: string; value: string; onChange: (value: string) => void }) {
  const [text, setText] = useState(value);
  useEffect(() => setText(value), [value]);
  return (
    <label className="appearance-swatch">
      <input
        type="color"
        aria-label={label}
        className="appearance-swatch-dot"
        value={value.toLowerCase()}
        onChange={(event) => onChange(event.currentTarget.value.toUpperCase())}
      />
      <span className="appearance-swatch-name">{label}</span>
      <input
        className="appearance-swatch-hex"
        aria-label={`${label} hex`}
        value={text}
        spellCheck={false}
        maxLength={7}
        onChange={(event) => {
          const next = event.currentTarget.value;
          setText(next);
          if (HEX.test(next)) onChange(next.toUpperCase());
        }}
      />
    </label>
  );
}

function FontSelect({ label, fonts, value, placeholder, onChange }: {
  label: string;
  fonts: string[];
  value?: string;
  placeholder: string;
  onChange: (value: string | undefined) => void;
}) {
  return (
    <div className="appearance-font">
      <Select
        aria-label={label}
        value={value ?? ""}
        onChange={(event) => onChange(event.currentTarget.value || undefined)}
      >
        <option value="">{placeholder}</option>
        {fonts.map((font) => (
          <option key={font} value={font}>{font}</option>
        ))}
      </Select>
    </div>
  );
}

function Slider({ label, min, max, step, value, display, onChange }: {
  label: string;
  min: number;
  max: number;
  step: number;
  value: number;
  display: string;
  onChange: (value: number) => void;
}) {
  return (
    <div className="appearance-slider">
      <input
        type="range"
        aria-label={label}
        min={min}
        max={max}
        step={step}
        value={value}
        onChange={(event) => onChange(Number(event.currentTarget.value))}
      />
      <span className="appearance-slider-value">{display}</span>
    </div>
  );
}

function ImageField({ url, file, chooseLabel, removeLabel, onChoose, onRemove }: {
  url: string | null;
  file?: string;
  chooseLabel: string;
  removeLabel: string;
  onChoose: () => void;
  onRemove?: () => void;
}) {
  return (
    <div className="appearance-image">
      {file && (
        <span className="appearance-thumb">{url ? <img alt="" src={url} /> : null}</span>
      )}
      {file && <span className="appearance-image-name">{file}</span>}
      <Button variant="secondary" size="sm" onClick={onChoose}>{chooseLabel}</Button>
      {onRemove && <Button variant="ghost" size="sm" onClick={onRemove}>{removeLabel}</Button>}
    </div>
  );
}

function CustomCard({ theme, inUse, selected, onSelect }: {
  theme: ThemeSummary;
  inUse: boolean;
  selected: boolean;
  onSelect: () => void;
}) {
  const { t } = useTranslation();
  const broken = theme.problems.length > 0;
  return (
    <button
      type="button"
      role="listitem"
      className="appearance-card"
      data-selected={selected || undefined}
      data-in-use={inUse || undefined}
      aria-pressed={selected}
      onClick={onSelect}
    >
      <span className="appearance-card-swatch" data-base={theme.base ?? "dark"} style={swatchStyle(theme.swatch)} aria-hidden="true">
        <span className="appearance-card-rail" />
        <span className="appearance-card-lines" />
      </span>
      <span className="appearance-card-name">
        <span className="appearance-card-title">{theme.name ?? theme.id}</span>
        {inUse && <span className="appearance-chip">{t("appearance.inUse")}</span>}
      </span>
      <span className="appearance-card-meta" data-broken={broken || undefined}>
        {broken ? t("appearance.needsFixing") : theme.author ?? theme.id}
      </span>
    </button>
  );
}

const SWATCH_HEX = /^#[0-9A-F]{6}$/;

/** The daemon's resolved colors as the thumbnail's own variables. */
function swatchStyle(swatch: Record<string, string> | undefined): CSSProperties | undefined {
  if (!swatch) return undefined;
  const vars: Record<string, string> = {};
  for (const [name, value] of Object.entries(swatch)) {
    if (SWATCH_HEX.test(value)) vars[`--swatch-${name}`] = value;
  }
  return vars as CSSProperties;
}

function BuiltInSwatch({ kind }: { kind: Theme }) {
  return (
    <span className="appearance-card-swatch" data-builtin={kind} aria-hidden="true">
      <span className="appearance-card-lines" />
    </span>
  );
}

/** A small sample of the home screen, painted with the draft's tokens. */
function ThemePreview({ tokens, background, sticker }: {
  tokens: Record<string, string>;
  background: { url: string; placement: "sidebar" | "window"; opacity: number } | null;
  sticker: { url: string; corner: ThemeStickerCorner; size: number } | null;
}) {
  const { t } = useTranslation();
  const style = useMemo(() => {
    const vars: Record<string, string> = {};
    for (const [name, value] of Object.entries(tokens)) {
      if (name.startsWith("--")) vars[name] = value;
    }
    return vars as CSSProperties;
  }, [tokens]);
  const bgStyle = background
    ? ({ backgroundImage: `url("${background.url}")`, opacity: background.opacity } as CSSProperties)
    : undefined;
  return (
    <div className="appearance-preview" style={style} aria-hidden="true">
      {background?.placement === "window" && <span className="appearance-preview-bg" data-placement="window" style={bgStyle} />}
      <div className="appearance-preview-sidebar">
        {background?.placement === "sidebar" && <span className="appearance-preview-bg" style={bgStyle} />}
        <span className="appearance-preview-nav" data-active="true">{t("appearance.sample.home")}</span>
        <span className="appearance-preview-nav">{t("appearance.sample.wiki")}</span>
        <span className="appearance-preview-nav">{t("appearance.sample.spaces")}</span>
      </div>
      <div className="appearance-preview-main">
        <span className="appearance-preview-title">{t("appearance.sample.title")}</span>
        <div className="appearance-preview-stats">
          <span><b>128</b>{t("appearance.sample.pages")}</span>
          <span><b>1,204</b>{t("appearance.sample.memories")}</span>
          <span><b>76</b>{t("appearance.sample.topics")}</span>
        </div>
        <span className="appearance-preview-section">{t("appearance.sample.inProgress")}</span>
        <div className="appearance-preview-card">
          {[t("appearance.sample.row1"), t("appearance.sample.row2"), t("appearance.sample.row3")].map((row, index) => (
            <span key={row} className="appearance-preview-row">
              <span className="appearance-preview-dot" />
              {row}
              {index === 0 && <span className="appearance-preview-tag">{t("appearance.sample.tag")}</span>}
            </span>
          ))}
        </div>
        <span className="appearance-preview-link">wenlan.app</span>
      </div>
      {sticker && (
        <img
          alt=""
          className="appearance-preview-sticker"
          data-corner={sticker.corner}
          src={sticker.url}
          style={{ maxWidth: sticker.size / 2.5, maxHeight: sticker.size / 2.5 }}
        />
      )}
    </div>
  );
}

function SparkleIcon() {
  return (
    <svg aria-hidden="true" width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinejoin="round">
      <path d="M12 3l1.8 5.2L19 10l-5.2 1.8L12 17l-1.8-5.2L5 10l5.2-1.8z" />
    </svg>
  );
}

function FolderIcon() {
  return (
    <svg aria-hidden="true" width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinejoin="round">
      <path d="M3 7a2 2 0 012-2h4l2 2h8a2 2 0 012 2v8a2 2 0 01-2 2H5a2 2 0 01-2-2z" />
    </svg>
  );
}
