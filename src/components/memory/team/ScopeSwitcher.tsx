// SPDX-License-Identifier: AGPL-3.0-only
// The switch at the top left between the person's Private pages and each
// team they are on. It exists only for someone on a team: a person who never
// joined one sees nothing here and never reads the word "team".
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { CaretUpDown, Check, LockSimple } from "@phosphor-icons/react";
import { teamLabel } from "../sharing/sharing";
import { JoinSharedDialog } from "../sharing/JoinSharedDialog";
import { SCOPE_PRIVATE, useScope } from "./scope";
import { TeamNameDialog, TeamPeopleDialog, useLeaveTeam } from "./TeamPage";
import "./scope-switcher.css";

function Mark({ name }: { readonly name: string | null }) {
  return (
    <span className="scope-mark" aria-hidden="true">
      {name === null ? <LockSimple size={12} weight="bold" /> : [...name.trim()][0]?.toUpperCase() ?? "?"}
    </span>
  );
}

export function ScopeSwitcher({ compact = false, onSwitched, onJoined }: {
  readonly compact?: boolean;
  /** After picking a side: the shell opens that side's home. */
  readonly onSwitched: () => void;
  /** After joining a team from here, with its key. */
  readonly onJoined: (teamKey: string) => void;
}) {
  const { t } = useTranslation();
  const scope = useScope();
  const [open, setOpen] = useState(false);
  const [dialog, setDialog] = useState<"people" | "invite" | "rename" | "join" | null>(null);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const away = (event: MouseEvent) => { if (!ref.current?.contains(event.target as Node)) setOpen(false); };
    const escape = (event: KeyboardEvent) => { if (event.key === "Escape") setOpen(false); };
    document.addEventListener("mousedown", away);
    document.addEventListener("keydown", escape);
    return () => {
      document.removeEventListener("mousedown", away);
      document.removeEventListener("keydown", escape);
    };
  }, [open]);
  const leave = useLeaveTeam(scope.team);
  if (scope.teams.length === 0) return null;

  const current = scope.team;
  const currentName = current ? teamLabel(t, current.team) : t("team.scope.private");
  const pick = (key: string) => {
    setOpen(false);
    scope.setScope(key);
    onSwitched();
  };
  const actions = !current ? [] : [
    { label: t("team.scope.people", { team: currentName }), onPick: () => setDialog("people") },
    ...(current.hosted
      ? [
          { label: t("team.scope.invite"), onPick: () => setDialog("invite") },
          { label: t("team.scope.rename", { team: currentName }), onPick: () => setDialog("rename") },
        ]
      : [{ label: t("team.scope.leave", { team: currentName }), onPick: () => void leave().then((left) => { if (left) scope.setScope(SCOPE_PRIVATE); }) }]),
  ];
  const sides = [
    { key: SCOPE_PRIVATE, name: t("team.scope.private"), mark: null as string | null },
    ...scope.teams.map((group) => {
      const name = teamLabel(t, group.team);
      return { key: group.key, name, mark: name };
    }),
  ];

  return (
    <div className={`scope-switcher${compact ? " scope-switcher--compact" : ""}`} ref={ref}>
      <button
        type="button"
        className="scope-switcher-button"
        aria-haspopup="menu"
        aria-expanded={open}
        aria-label={t("team.scope.switchLabel", { current: currentName })}
        title={compact ? currentName : undefined}
        onClick={() => setOpen(!open)}
      >
        <Mark name={current ? currentName : null} />
        {!compact && <span className="scope-switcher-name">{currentName}</span>}
        {!compact && <CaretUpDown className="scope-switcher-caret" size={14} aria-hidden="true" />}
      </button>
      {open && (
        <ul className="scope-switcher-menu" role="menu">
          {sides.map((side) => {
            const chosen = (current?.key ?? SCOPE_PRIVATE) === side.key;
            return (
              <li role="none" key={side.key}>
                <button type="button" role="menuitemradio" aria-checked={chosen} onClick={() => pick(side.key)}>
                  <Mark name={side.mark} />
                  <span className="scope-switcher-name">{side.name}</span>
                  {chosen && <Check className="scope-switcher-check" size={14} aria-hidden="true" />}
                </button>
              </li>
            );
          })}
          {/* The shown team's own things: its people, inviting, its name,
              leaving. They live here, not on a page, so home stays the same
              home on every side. */}
          {current && actions.map((action, index) => (
            <li role="none" key={action.label} className={index === 0 ? "scope-switcher-gap" : undefined}>
              <button type="button" role="menuitem" onClick={() => { setOpen(false); action.onPick(); }}>
                {action.label}
              </button>
            </li>
          ))}
          <li role="none" className={current ? undefined : "scope-switcher-gap"}>
            <button type="button" role="menuitem" onClick={() => { setOpen(false); setDialog("join"); }}>
              {t("team.scope.join")}
            </button>
          </li>
        </ul>
      )}
      {(dialog === "people" || dialog === "invite") && current && (
        <TeamPeopleDialog group={current} inviteFirst={dialog === "invite"} onClose={() => setDialog(null)} />
      )}
      {dialog === "rename" && current && <TeamNameDialog group={current} onClose={() => setDialog(null)} />}
      {dialog === "join" && (
        <JoinSharedDialog
          only="code"
          onClose={() => setDialog(null)}
          onJoined={(kind, target) => {
            setDialog(null);
            if (kind === "team") onJoined(`team:${target}`);
          }}
        />
      )}
    </div>
  );
}
