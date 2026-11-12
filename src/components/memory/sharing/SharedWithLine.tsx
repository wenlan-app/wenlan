// SPDX-License-Identifier: AGPL-3.0-only
// The one quiet line under a shared Space's or page's title: who it is shared
// with. It speaks up only when the folder is missing.
import { useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { sharePage, shareSpace, type SharedStatus } from "../../../lib/tauri";
import { SHARES_KEY, teamLabel, teamPausedText, withLine } from "./sharing";
import { useTeamStatus } from "./team";
import { teamAccessText } from "./TeamMembers";
import "./sharing.css";

export function SharedWithLine({ share, onOpen }: { readonly share: SharedStatus; readonly onOpen: () => void }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [pending, setPending] = useState(false);
  // A team Space waits for its server, so there is no folder to choose.
  if (share.last?.paused && share.host) {
    return (
      <p className="shared-with-line shared-with-paused" role="status">
        <button type="button" className="shared-with-button" onClick={onOpen}><span>{teamPausedText(t, share)}</span></button>
      </p>
    );
  }
  if (share.last?.paused) {
    const choose = async () => {
      const chosen = await openDialog({ directory: true, multiple: false, defaultPath: share.folder });
      if (typeof chosen !== "string") return;
      setPending(true);
      try {
        if (share.kind === "space") await shareSpace(share.target, chosen);
        else await sharePage(share.target, chosen);
      } catch {
        // The line stays until a pass finds the folder.
      } finally {
        setPending(false);
        await queryClient.invalidateQueries({ queryKey: SHARES_KEY });
      }
    };
    return (
      <p className="shared-with-line shared-with-paused" role="status">
        <span>{t("sharing.pausedLine")}</span>
        <button type="button" className="share-link" disabled={pending} onClick={() => void choose()}>{t("sharing.chooseFolder")}</button>
      </p>
    );
  }
  return (
    <p className="shared-with-line">
      <button type="button" className="shared-with-button" onClick={onOpen}>
        <PeopleIcon />
        {share.hosted && share.kind === "space"
          ? <HostedTeamLine share={share} />
          : <span>{share.host ? `${teamLabel(t, share.team)} · ${t(share.read_only ? "team.readOnly" : "team.canEdit")}` : withLine(share, t)}</span>}
      </button>
    </p>
  );
}

/** A Space this computer hosts for its team: the team and who can use it. */
function HostedTeamLine({ share }: { readonly share: SharedStatus }) {
  const { t } = useTranslation();
  const status = useTeamStatus();
  return <span>{`${teamLabel(t, share.team ?? status.data?.team)} · ${teamAccessText(t, share.target, status.data)}`}</span>;
}

export function PeopleIcon({ className = "shared-people-icon" }: { readonly className?: string }) {
  return (
    <svg aria-hidden="true" className={className} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6">
      <circle cx="9" cy="9" r="3.2" />
      <path d="M3.5 18.5c.6-2.9 2.8-4.5 5.5-4.5s4.9 1.6 5.5 4.5" />
      <circle cx="16.5" cy="9.5" r="2.6" />
      <path d="M16 14.1c2.3.1 4 1.5 4.5 4.4" />
    </svg>
  );
}
