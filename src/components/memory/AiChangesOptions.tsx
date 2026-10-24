// SPDX-License-Identifier: AGPL-3.0-only
import type { ReactNode } from "react";
import "./AiChangesOptions.css";

export type AiChangesOption<V extends string> = { value: V; label: string; hint: string };

/**
 * The one "AI changes" choice, drawn the same way for a Wenlan page and for a
 * note in the person's vault: a heading, one row per option with its hint, and
 * a grey line under them that says where the current choice comes from.
 */
export function AiChangesOptions<V extends string>({ heading, name, options, value, disabled, onChange, children }: {
  heading: string;
  name: string;
  options: readonly AiChangesOption<V>[];
  value: V;
  disabled?: boolean;
  onChange: (value: V) => void;
  children?: ReactNode;
}) {
  return <section className="note-ai-changes" aria-label={heading}>
    <h3 className="note-info-section-heading">{heading}</h3>
    <div className="note-ai-options" role="radiogroup" aria-label={heading}>
      {options.map((option) => {
        const on = value === option.value;
        return <label key={option.value} className={`note-ai-option${on ? " is-on" : ""}`}>
          <input type="radio" name={name} checked={on} disabled={disabled} onChange={() => onChange(option.value)} />
          <span>
            <span className="note-ai-option-label">{option.label}</span>
            <span className="note-ai-option-hint">{option.hint}</span>
          </span>
        </label>;
      })}
    </div>
    {children}
  </section>;
}
