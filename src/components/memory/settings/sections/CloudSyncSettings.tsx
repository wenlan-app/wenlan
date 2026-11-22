// SPDX-License-Identifier: AGPL-3.0-only
// Settings › Sync › Wenlan cloud: connect this computer, and the whole
// library is kept in the person's Wenlan cloud too.
import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { open as openExternal } from "@tauri-apps/plugin-shell";
import { cloudStatus, connectCloud, disconnectCloud, type CloudStatus } from "../../../../lib/tauri";
import { ConfirmDialog } from "../../../ConfirmDialog";
import { relativeMs } from "../../page/format";
import { SHARES_KEY } from "../../sharing/sharing";
import { SettingsGroup } from "../SettingsView";
import { Button, SettingRow } from "../primitives";
import "../../sharing/sharing.css";
import "./librarySync.css";

export const CLOUD_KEY = ["cloud-status"] as const;

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function host(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return url;
  }
}

export default function CloudSyncSettings() {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const queryClient = useQueryClient();
  const status = useQuery<CloudStatus>({
    queryKey: CLOUD_KEY,
    queryFn: cloudStatus,
    // Quickly while the browser is being answered; slowly after.
    refetchInterval: (query) => (query.state.data?.state === "waiting" ? 2000 : 30000),
  });
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const data = status.data;

  // Once connected, the library list shows the cloud's computers too.
  const was = useRef(data?.state);
  useEffect(() => {
    if (was.current === "waiting" && data?.state === "connected") {
      void queryClient.invalidateQueries({ queryKey: SHARES_KEY });
    }
    was.current = data?.state;
  }, [data?.state, queryClient]);

  const refresh = () => queryClient.invalidateQueries({ queryKey: CLOUD_KEY });
  const connect = async () => {
    setBusy(true);
    setError(null);
    try {
      const start = await connectCloud();
      await refresh();
      await openExternal(start.verification_uri).catch(() => undefined);
    } catch (caught) {
      setError(errorText(caught));
    } finally {
      setBusy(false);
    }
  };
  const disconnect = async () => {
    setError(null);
    try {
      queryClient.setQueryData(CLOUD_KEY, await disconnectCloud());
    } catch (caught) {
      setError(errorText(caught));
    }
  };

  const title = t("cloudSync.title");
  const badge = t("cloudSync.trial");
  if (!data) {
    return (
      <SettingsGroup className="mem-fade-up" style={{ animationDelay: "0ms" }}>
        <SettingRow title={title} titleBadge={badge} description={t("cloudSync.hint")} control={null} />
      </SettingsGroup>
    );
  }

  if (data.state === "waiting" && data.start) {
    const start = data.start;
    return (
      <SettingsGroup className="mem-fade-up" style={{ animationDelay: "0ms" }}>
        <SettingRow
          title={title}
          titleBadge={badge}
          description={t("cloudSync.waiting", { code: start.user_code })}
          controlLayout="inline"
          control={(
            <div className="library-sync-buttons">
              <Button variant="secondary" size="sm" onClick={() => void openExternal(start.verification_uri).catch(() => undefined)}>
                {t("cloudSync.openAgain")}
              </Button>
              <Button variant="secondary" size="sm" onClick={() => void disconnect()}>
                {t("cloudSync.cancel")}
              </Button>
            </div>
          )}
        />
      </SettingsGroup>
    );
  }

  if (data.state === "connected") {
    const line = data.last_ok_at
      ? t("cloudSync.synced", { when: relativeMs(Date.parse(data.last_ok_at), locale) })
      : t("cloudSync.syncing");
    const problem = error ?? data.error ?? null;
    return (
      <SettingsGroup className="mem-fade-up" style={{ animationDelay: "0ms" }}>
        <SettingRow
          title={title}
          titleBadge={t("cloudSync.connected")}
          description={line}
          error={problem ? t("cloudSync.failed", { message: problem }) : null}
          controlLayout="inline"
          control={(
            <Button variant="secondary" size="sm" onClick={() => setConfirming(true)}>
              {t("cloudSync.disconnect")}
            </Button>
          )}
        />
        <SettingRow
          title={t("cloudSync.web")}
          description=""
          controlLayout="inline"
          control={(
            <button type="button" className="share-link" onClick={() => void openExternal(data.url).catch(() => undefined)}>
              {host(data.url)}
            </button>
          )}
        />
        {confirming && (
          <ConfirmDialog
            message={t("cloudSync.disconnectConfirm")}
            confirmLabel={t("cloudSync.disconnectButton")}
            cancelLabel={t("cloudSync.cancel")}
            onAnswer={(yes) => {
              setConfirming(false);
              if (yes) void disconnect();
            }}
          />
        )}
      </SettingsGroup>
    );
  }

  const problem = error ?? data.error ?? null;
  return (
    <SettingsGroup className="mem-fade-up" style={{ animationDelay: "0ms" }}>
      <SettingRow
        title={title}
        titleBadge={badge}
        description={t("cloudSync.hint")}
        error={problem ? t("cloudSync.notConnected", { message: problem }) : null}
        controlLayout="inline"
        control={(
          <Button variant="primary" size="sm" disabled={busy} onClick={() => void connect()}>
            {t("cloudSync.signIn")}
          </Button>
        )}
      />
    </SettingsGroup>
  );
}
