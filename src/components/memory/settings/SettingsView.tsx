// SPDX-License-Identifier: AGPL-3.0-only
import type { ReactNode } from "react";
import { CollectionFrame } from "../CollectionLayout";
import { SectionHeader, Card } from "./primitives";
import "./settingsView.css";

export function SettingsView({ title, backButton, wide, children }: {
  title: string;
  backButton?: ReactNode;
  /** Use the full width, for a section with a side-by-side preview. */
  wide?: boolean;
  children: ReactNode;
}) {
  return (
    <CollectionFrame className={`settings-view settings-page${wide ? " settings-view--wide" : ""}`}>
      <div className="settings-view-content">
        <header className="settings-view-heading">
          {backButton}
          <h1>{title}</h1>
        </header>
        <div className="settings-view-groups">{children}</div>
      </div>
    </CollectionFrame>
  );
}

export function SettingsGroup({ heading, children, className, style }: {
  heading?: string;
  children: ReactNode;
  className?: string;
  style?: React.CSSProperties;
}) {
  return (
    <section className={`settings-view-group${className ? ` ${className}` : ""}`} style={style}>
      {heading && <SectionHeader label={heading} />}
      <Card padding="rows">{children}</Card>
    </section>
  );
}
