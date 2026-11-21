// SPDX-License-Identifier: AGPL-3.0-only
// `/app/device`: the member types the short code `wenlan login` printed,
// sees which AI tool asks and what it can use, and allows or refuses it.
// Allowing needs a sign-in made just now; the page then sends the browser
// to the provider and picks up the same code when it comes back.
import { useEffect, useRef, useState, type FormEvent } from "react";
import { useTranslation } from "react-i18next";
import { deviceDecide, deviceLookup, reauthThen, type DeviceRequest, type TeamMe } from "./team";
import "./teamWeb.css";

const ALPHABET = /[ABCDEFGHJKMNPQRSTUVWXYZ23456789]/;
const LENGTH = 8;

type Step =
  | { kind: "code" }
  | { kind: "consent"; code: string; request: DeviceRequest }
  | { kind: "done"; tool: string; allowed: boolean };

function clean(text: string): string {
  return [...text.toUpperCase()].filter((c) => ALPHABET.test(c)).join("").slice(0, LENGTH);
}

function providerName(provider: string | undefined): string {
  return provider === "github" ? "GitHub" : "Google";
}

export function DevicePage({ me, initialCode }: { readonly me: TeamMe; readonly initialCode?: string }) {
  const { t } = useTranslation();
  const team = me.team?.name ?? t("teamWeb.teamFallback");
  const letter = [...team.trim()][0]?.toUpperCase() ?? "?";
  const provider = me.signed_in_with === "browser" ? me.accounts?.[0]?.provider : undefined;
  const [code, setCode] = useState(clean(initialCode ?? ""));
  const [step, setStep] = useState<Step>({ kind: "code" });
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const input = useRef<HTMLInputElement>(null);
  const started = useRef(false);

  const errorFor = (reason: string) =>
    reason === "unknown_code" ? t("teamWeb.deviceWrong") : reason === "too_many_tries" ? t("teamWeb.deviceTooMany") : t("teamWeb.deviceFailed");

  const look = async (typed: string) => {
    setBusy(true);
    setError(null);
    const found = await deviceLookup(typed);
    setBusy(false);
    if (found.ok) setStep({ kind: "consent", code: typed, request: found.value });
    else setError(errorFor(found.error));
  };

  // Back from signing in again with the code in hand: straight to consent.
  useEffect(() => {
    if (started.current) return;
    started.current = true;
    if (code.length === LENGTH) void look(code);
    else input.current?.focus();
    // Only once, on arrival.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (code.length === LENGTH && !busy) void look(code);
  };

  const decide = async (allow: boolean) => {
    if (step.kind !== "consent") return;
    setBusy(true);
    setError(null);
    const decided = await deviceDecide(step.code, allow);
    if (decided.ok) {
      setBusy(false);
      setStep({ kind: "done", tool: step.request.tool, allowed: allow });
      return;
    }
    if (decided.error === "reauth_required" && provider) {
      if (await reauthThen(provider, { kind: "device", code: step.code })) return;
    }
    setBusy(false);
    setError(errorFor(decided.error));
  };

  return (
    <main className="team-signin" data-testid="team-device">
      <div className="team-signin-column">
        <span className="team-signin-mark" aria-hidden="true">{letter}</span>
        {step.kind === "code" && (
          <form onSubmit={submit}>
            <h1 className="team-signin-title">{t("teamWeb.deviceTitle")}</h1>
            <p className="team-signin-hint">{t("teamWeb.deviceHint")}</p>
            <label className="team-device-code">
              <span className="team-device-label">{t("teamWeb.deviceCode")}</span>
              <input
                ref={input}
                data-testid="team-device-input"
                value={code.length > 4 ? `${code.slice(0, 4)}-${code.slice(4)}` : code}
                onChange={(event) => setCode(clean(event.target.value))}
                autoComplete="one-time-code"
                autoCapitalize="characters"
                spellCheck={false}
                inputMode="text"
                maxLength={LENGTH + 1}
                placeholder="ABCD-EFGH"
              />
            </label>
            <button type="submit" className="team-device-primary" data-testid="team-device-continue" disabled={code.length !== LENGTH || busy}>
              {t("teamWeb.deviceContinue")}
            </button>
          </form>
        )}
        {step.kind === "consent" && (
          <>
            <h1 className="team-signin-title">{t("teamWeb.consentTitle", { tool: step.request.tool, team })}</h1>
            <p className="team-device-quiet">{t("teamWeb.consentNamed", { tool: step.request.tool })}</p>
            <p className="team-signin-hint">{t("teamWeb.consentSpaces")}</p>
            <ul className="team-device-spaces" data-testid="team-device-spaces">
              {step.request.spaces.map((entry) => (
                <li key={entry.space}>{entry.access === "read" ? t("teamWeb.consentReadOnly", { space: entry.space }) : entry.space}</li>
              ))}
            </ul>
            <p className="team-signin-hint">{t("teamWeb.consentCant")}<br />{t("teamWeb.consentRemove")}</p>
            <div className="team-device-actions">
              <button type="button" className="team-device-secondary" data-testid="team-device-cancel" disabled={busy} onClick={() => void decide(false)}>
                {t("teamWeb.consentCancel")}
              </button>
              <button type="button" className="team-device-primary" data-testid="team-device-allow" disabled={busy} onClick={() => void decide(true)}>
                {t("teamWeb.consentAllow")}
              </button>
            </div>
            {provider && <p className="team-device-quiet">{t("teamWeb.consentConfirm", { provider: providerName(provider) })}</p>}
          </>
        )}
        {step.kind === "done" && (
          <>
            <h1 className="team-signin-title" data-testid="team-device-done">
              {step.allowed ? t("teamWeb.doneTitle", { tool: step.tool }) : t("teamWeb.deniedTitle", { tool: step.tool })}
            </h1>
            <p className="team-signin-hint">{step.allowed ? t("teamWeb.doneHint") : t("teamWeb.deniedHint")}</p>
          </>
        )}
        {error && <p className="team-account-error" role="alert">{error}</p>}
        {step.kind === "code" && <p className="team-device-quiet">{t("teamWeb.deviceSignedIn", { name: me.accounts?.[0]?.email ? `${me.name} · ${me.accounts[0].email}` : me.name })}</p>}
      </div>
    </main>
  );
}
