// SPDX-License-Identifier: AGPL-3.0-only
import { useId, useState } from "react";
import { useTranslation } from "react-i18next";
import type { Space } from "../../lib/tauri";
import { Button, Field, Select } from "./settings/primitives";
import { wholeLibrary, type RemoteScope } from "./remoteScope";

/**
 * The one question Web access asks: the whole library, or one Space. Nothing is
 * preselected. The whole library is one click; one Space reveals a picker first.
 *
 * `current` is what is shared now, when changing it; that choice is not offered again.
 */
export function RemoteScopeChooser({ spaces, defaultSpace, current = null, busy = false, disabled = false, size = "sm", onChoose }: {
  spaces: Space[];
  defaultSpace: string;
  current?: RemoteScope | null;
  busy?: boolean;
  disabled?: boolean;
  size?: "sm" | "md";
  onChoose: (scope: RemoteScope) => void;
}) {
  const { t } = useTranslation();
  const questionId = useId();
  const selectId = useId();
  const currentSpace = current?.kind === "space" ? current.name : null;
  const offered = spaces.filter((item) => item.name !== currentSpace);
  const [picking, setPicking] = useState(false);
  const [picked, setPicked] = useState<string | null>(null);
  const [pressed, setPressed] = useState<"whole" | "space" | null>(null);
  const space = picked !== null && offered.some((item) => item.name === picked)
    ? picked
    : offered.some((item) => item.name === defaultSpace) ? defaultSpace : offered[0]?.name ?? "";
  const blocked = busy || disabled;

  const choose = (scope: RemoteScope) => {
    setPressed(scope.kind === "wholeLibrary" ? "whole" : "space");
    onChoose(scope);
  };

  return (
    <div role="group" aria-labelledby={questionId} className="min-w-0 space-y-3">
      <p id={questionId} className="text-sm font-medium text-[var(--mem-text)]">{t("remoteAccess.scopeQuestion")}</p>
      <div className="flex flex-wrap gap-2">
        <Button variant="primary" size={size} loading={busy && pressed === "whole"}
          disabled={blocked || current?.kind === "wholeLibrary"} onClick={() => choose(wholeLibrary)}>
          {t("remoteAccess.shareWholeLibrary")}
        </Button>
        <Button variant="secondary" size={size} aria-expanded={picking}
          disabled={blocked || offered.length === 0} onClick={() => setPicking((value) => !value)}>
          {t("remoteAccess.shareOneSpace")}
        </Button>
      </div>
      {picking && offered.length > 0 && (
        <div className="flex flex-wrap items-end gap-2">
          <div className="min-w-0 w-full max-w-xs">
            <Field label={t("remoteAccess.spaceToShare")} htmlFor={selectId}>
              <Select size={size} value={space} disabled={blocked} onChange={(event) => setPicked(event.target.value)}>
                {offered.map((item) => <option key={item.id} value={item.name}>{item.name}</option>)}
              </Select>
            </Field>
          </div>
          <Button variant="primary" size={size} loading={busy && pressed === "space"} disabled={blocked || space === ""}
            onClick={() => choose({ kind: "space", name: space })}>
            {t("remoteAccess.shareThisSpace")}
          </Button>
        </div>
      )}
    </div>
  );
}
