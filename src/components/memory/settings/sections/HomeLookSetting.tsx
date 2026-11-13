// SPDX-License-Identifier: AGPL-3.0-only
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { clipboardWrite, removePageLook, restorePageLook } from "../../../../lib/tauri";
import { pageLooksEnabled } from "../../page/look/lookFeature";
import { usePageLook } from "../../page/look/usePageLook";
import { HOME_LOOK_ID, useHomeLookFollowsTheme } from "../../pages/HomeLook";
import { Button } from "../primitives";

/**
 * The theme in use's home (UX-LOOK-10), under the theme cards: home's HTML
 * view is part of the theme, so choosing another theme is how home goes back,
 * and this row only takes the view off (with Undo). Without a view it says
 * how to ask an AI tool for one.
 */
export default function HomeLookSetting() {
  const { t } = useTranslation();
  const enabled = pageLooksEnabled();
  const look = usePageLook({ pageId: HOME_LOOK_ID, supported: enabled, version: 0 });
  useHomeLookFollowsTheme(look.refresh);
  if (!enabled) return null;

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
          <p className="appearance-home-ask">{t("pageLook.homeHtml")}</p>
          <Button variant="ghost" size="sm" onClick={() => void remove()}>
            {t("pageLook.homeRemove")}
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
