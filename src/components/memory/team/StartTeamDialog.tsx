// SPDX-License-Identifier: AGPL-3.0-only
// Settings › Team › "Start a team on this computer": this computer becomes
// the team's server. It asks three things once: the team's name, the
// owner's own name (how the team sees what they write here) and the address
// the others reach this computer at. The team listener then stays on, after
// restarts too, until it is turned off in the same place.
// On Windows, Windows Firewall blocks other computers from a new listener on
// its own, without asking, on a network it calls public. So before the
// listener starts, Windows asks its administrator once to let teammates in.
import { useEffect, useRef, useState, type FormEvent } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { getProfile, teamFirewall, teamFirewallAllow, teamStart, type TeamStatus } from "../../../lib/tauri";
import { SHARES_KEY } from "../sharing/sharing";
import { TEAM_FIREWALL_KEY, TEAM_KEY } from "../sharing/team";
import "../sharing/sharing.css";

export function StartTeamDialog({ status, onClose, onStarted }: {
  readonly status: TeamStatus | undefined;
  readonly onClose: () => void;
  readonly onStarted: () => void;
}) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const profile = useQuery({ queryKey: ["profile"], queryFn: getProfile });
  const dialogRef = useRef<HTMLDialogElement>(null);
  const suggested = status?.suggested_urls ?? [];
  const [name, setName] = useState(status?.team?.name ?? "");
  const [person, setPerson] = useState<string | null>(null);
  const [address, setAddress] = useState(status?.url || suggested[0] || "");
  const [pending, setPending] = useState(false);
  const [askingWindows, setAskingWindows] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    const dialog = dialogRef.current;
    if (dialog && !dialog.open) dialog.showModal();
    return () => dialog?.close();
  }, []);
  const yourName = person ?? profile.data?.display_name ?? "";
  const ready = name.trim() !== "" && yourName.trim() !== "" && address.trim() !== "" && !pending;

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (!ready) return;
    setPending(true);
    setError(null);
    try {
      // Saying no here does not stop the team: Settings › Team then says
      // other computers can't connect, with the same button.
      const wall = await teamFirewall().catch(() => null);
      if (wall && !wall.open) {
        setAskingWindows(true);
        await teamFirewallAllow(wall).catch(() => undefined);
        setAskingWindows(false);
      }
      await teamStart(name.trim(), yourName.trim(), address.trim());
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: TEAM_KEY }),
        queryClient.invalidateQueries({ queryKey: SHARES_KEY }),
        queryClient.invalidateQueries({ queryKey: ["profile"] }),
        queryClient.invalidateQueries({ queryKey: TEAM_FIREWALL_KEY }),
      ]);
      onStarted();
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
    } finally {
      setPending(false);
      setAskingWindows(false);
    }
  };

  return (
    <dialog
      ref={dialogRef}
      className="share-dialog"
      aria-labelledby="start-team-title"
      onCancel={(event) => { event.preventDefault(); if (!pending) onClose(); }}
    >
      <form onSubmit={(event) => void submit(event)}>
        <h2 id="start-team-title">{t("team.start.title")}</h2>
        <label className="share-field">
          <span className="share-label">{t("team.start.teamName")}</span>
          <input className="share-input" value={name} placeholder={t("team.page.nameTeamPlaceholder")} maxLength={60} onChange={(event) => setName(event.target.value)} disabled={pending} autoFocus />
        </label>
        <label className="share-field">
          <span className="share-label">{t("team.start.yourName")}</span>
          <input className="share-input" value={yourName} maxLength={80} onChange={(event) => setPerson(event.target.value)} disabled={pending} />
          <span className="share-hint">{t("team.start.yourNameHint")}</span>
        </label>
        <label className="share-field">
          <span className="share-label">{t("team.start.address")}</span>
          <input className="share-input" value={address} placeholder={t("team.urlPlaceholder")} spellCheck={false} list="start-team-addresses" onChange={(event) => setAddress(event.target.value)} disabled={pending} />
          {suggested.length > 1 && (
            <datalist id="start-team-addresses">
              {suggested.map((url) => <option key={url} value={url} />)}
            </datalist>
          )}
          <span className="share-hint">{suggested.length > 0 ? t("team.start.addressHint") : t("team.start.addressNone")}</span>
        </label>
        <p className="share-hint">{t("team.start.stayOn")}</p>
        {error && <p className="share-warning" role="alert">{error}</p>}
        <div className="share-actions">
          <button type="button" className="share-secondary" onClick={onClose} disabled={pending}>{t("sharing.cancel")}</button>
          <button type="submit" className="share-primary" disabled={!ready}>{askingWindows ? t("team.start.askingWindows") : pending ? t("team.start.starting") : t("team.start.action")}</button>
        </div>
      </form>
    </dialog>
  );
}
