// SPDX-License-Identifier: AGPL-3.0-only
import { useTranslation } from "react-i18next";
import { useDaemonStartupPhase } from "../../lib/daemonStartup";

/** One quiet line while the workspace waits for the daemon to answer
 *  (src/lib/daemonStartup.ts). It goes as soon as the daemon answers; a
 *  daemon that never does is the App banner's to report, not this line's. */
export default function DaemonStartupStatus({ compact = false }: { readonly compact?: boolean }) {
  const { t } = useTranslation();
  const phase = useDaemonStartupPhase();
  if (phase !== "starting") return null;
  const label = t("boot.startingInBackground");
  return (
    <div
      role="status"
      aria-label={compact ? label : undefined}
      title={compact ? label : undefined}
      data-testid="daemon-startup-status"
      className={`mem-daemon-startup-status${compact ? " mem-daemon-startup-status--compact" : ""}`}
    >
      <span aria-hidden="true" className="mem-daemon-startup-dot" />
      {!compact && <span className="mem-daemon-startup-label">{label}</span>}
    </div>
  );
}
