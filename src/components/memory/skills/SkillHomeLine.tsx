// SPDX-License-Identifier: AGPL-3.0-only
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import {
  listSkillBorrowRecords,
  skillsFromLibraryEnabled,
  type Page,
  type SkillBorrowRecord,
} from "../../../lib/tauri";
import { HOW_I_WORK_QUERY_KEYS, howIWorkSpaceName, startOfLocalWeek } from "./howIWorkModel";
import "./howIWork.css";

type Notice = {
  readonly pageId: string;
  readonly at: number;
  readonly record?: SkillBorrowRecord;
};

export function SkillHomeLine({ pages, onOpenPage }: {
  readonly pages: readonly Page[];
  readonly onOpenPage: (pageId: string) => void;
}) {
  const { t } = useTranslation();
  const enabled = useQuery({
    queryKey: HOW_I_WORK_QUERY_KEYS.enabled,
    queryFn: skillsFromLibraryEnabled,
    staleTime: 30_000,
    retry: false,
  });
  const records = useQuery({
    queryKey: ["skill-home-records", ...pages.map((page) => `${page.id}:${page.version}`)],
    queryFn: async () => Promise.all(pages.map(async (page) => ({
      page,
      records: await listSkillBorrowRecords(howIWorkSpaceName(page)),
    }))),
    enabled: enabled.data === true && pages.length > 0,
    retry: false,
  });

  if (enabled.data !== true) return null;
  const weekStart = startOfLocalWeek();
  const notices: Notice[] = [];
  for (const entry of records.data ?? []) {
    for (const record of entry.records) {
      if (record.self_report === "changed_plan" && record.at >= weekStart) {
        notices.push({ pageId: entry.page.id, at: record.at, record });
      }
    }
    const createdAt = Math.floor(Date.parse(entry.page.created_at) / 1000);
    if (Number.isFinite(createdAt) && createdAt >= weekStart) {
      notices.push({ pageId: entry.page.id, at: createdAt });
    }
  }
  const notice = notices.sort((left, right) => right.at - left.at)[0];
  if (!notice) return null;

  return (
    <p className="skill-home-line" data-testid="skill-home-line">
      {t("howIWork.home.line")}{" · "}
      <button type="button" onClick={() => onOpenPage(notice.pageId)}>{t("howIWork.home.open")}</button>
    </p>
  );
}
