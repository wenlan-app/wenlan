// SPDX-License-Identifier: AGPL-3.0-only
import { useTranslation } from "react-i18next";
import { ProviderMark } from "./ProviderMarks";
import "./teamWeb.css";

const KNOWN = ["google", "github"];

/**
 * `/app` with no working credential, on a server that signs people in with
 * Google or GitHub. Each button is a plain link: the server's start route
 * sends the browser on to the provider.
 */
export function SignInPage({ team, providers }: { readonly team: string | null; readonly providers: readonly string[] }) {
  const { t } = useTranslation();
  const name = team ?? t("teamWeb.teamFallback");
  const letter = [...name.trim()][0]?.toUpperCase() ?? "?";
  return (
    <main className="team-signin" data-testid="team-web-signin">
      <div className="team-signin-column">
        <span className="team-signin-mark" aria-hidden="true">{letter}</span>
        <h1 className="team-signin-title">{t("teamWeb.signInTitle", { team: name })}</h1>
        <p className="team-signin-hint">{t("teamWeb.signInHint")}</p>
        <div className="team-signin-buttons">
          {providers.filter((provider) => KNOWN.includes(provider)).map((provider) => (
            <a
              key={provider}
              className="team-signin-button"
              data-testid={`team-web-signin-${provider}`}
              href={`/auth/start/${provider}?purpose=signin`}
            >
              <ProviderMark provider={provider} />
              {provider === "google" ? t("teamWeb.signInWithGoogle") : t("teamWeb.signInWithGitHub")}
            </a>
          ))}
        </div>
        <p className="team-signin-new">{t("teamWeb.newHere")}</p>
      </div>
    </main>
  );
}
