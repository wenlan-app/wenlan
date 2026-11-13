// SPDX-License-Identifier: AGPL-3.0-only
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { clipboardWrite, removePageLook, restorePageLook } from "../../../../lib/tauri";
import { pageLooksEnabled } from "../../page/look/lookFeature";
import { usePageLook, type LookMode } from "../../page/look/usePageLook";
import { HOME_LOOK_ID, storedHomeMode, storeHomeMode } from "../../pages/HomeLook";
import { Button, SegmentedControl } from "../primitives";

/**
 * Settings › Appearance › Home (UX-LOOK-10): the one place home's HTML view
 * is switched or taken off, so home itself carries no extra control. Without
 * a view it only says how to ask an AI tool for one.
 */
export default function HomeLookSetting() {
  const { t } = useTranslation();
  const enabled = pageLooksEnabled();
  const look = usePageLook({ pageId: HOME_LOOK_ID, supported: enabled, version: 0, startMode: storedHomeMode });
  if (!enabled) return null;
  const lookId = look.look?.id ?? null;
  const mode = look.mode;

  const choose = (next: LookMode) => {
    if (lookId !== null) storeHomeMode(next, lookId);
    look.choose(next);
  };

  const copyAsk = async () => {
    try {
      await clipboardWrite(t("pageLook.homeAskPrompt"));
      toast.success(t("pageLook.copied"));
    } catch {
      toast.error(t("pageLook.copyFailed"));
    }
  };

  const remove = async () => {
    try {
      const removedId = await removePageLook(HOME_LOOK_ID);
      await look.refresh();
      toast(t("pageLook.removed"), {
        action: {
          label: t("pageLook.undo"),
          onClick: () => {
            void restorePageLook(HOME_LOOK_ID, removedId)
              .then(() => look.refresh())
              .catch(() => toast.error(t("pageLook.restoreFailed")));
          },
        },
      });
    } catch {
      toast.error(t("pageLook.removeFailed"));
    }
  };

  return (
    <section className="appearance-home" aria-labelledby="appearance-home-label" data-testid="home-look-setting">
      <h2 id="appearance-home-label" className="appearance-label">{t("pageLook.homeSetting")}</h2>
      {look.hasLook ? (
        <div className="appearance-home-row">
          <SegmentedControl<LookMode>
            aria-label={t("pageLook.homeSetting")}
            value={mode}
            onChange={choose}
            options={[
              { value: "text", label: t("pageLook.homeUsual") },
              { value: "look", label: t("pageLook.homeHtml") },
            ]}
          />
          <Button variant="ghost" size="sm" onClick={() => void remove()}>
            {t("pageLook.remove")}
          </Button>
        </div>
      ) : (
        <div className="appearance-home-row">
          <p className="appearance-home-ask">{t("pageLook.homeAsk")}</p>
          <Button variant="ghost" size="sm" onClick={() => void copyAsk()}>
            {t("pageLook.homeAskCopy")}
          </Button>
        </div>
      )}
    </section>
  );
}
