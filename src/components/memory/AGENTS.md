# AGENTS.md - src/components/memory/

## OVERVIEW

Main product UI cluster: home, search, spaces, memories, pages, citations,
sources, settings, remote access, imports, profile, and review surfaces.

## WHERE TO LOOK

| Task | Location | Notes |
| --- | --- | --- |
| Navigation or view state | `Main.tsx` | high blast radius; many query invalidations |
| Page citations | `PageDetail.tsx`, `page/PageInfo.tsx`, `page/CitationChip.tsx` | verified/unverified states and source ordering |
| Settings/config | `SettingsPage.tsx`, `settings/` | profile, capture, sources, diagnostics, remote access |
| Sources UI | `SourcesView.tsx`, `sources/` | add/list/sync source workflows |
| Memory rendering | `MemoryCard.tsx`, `ContentRenderer.tsx`, `MemoryDetail.tsx` | classifier/rendering invariants |
| Remote access UI | `RemoteAccessPanel.tsx`, `RemoteAccessApps.tsx`, `PairingApprovalDialog.tsx`, `RemoteAccessNotifier.tsx`, `useRemoteAccess.ts` | Settings panel, connected-apps list, global approval dialog (opened by `wenlan://pair` or "Have a code?"), stopped/expired notifications; talk to Rust remote-access commands. App identity comes from `lib/remoteClient.ts` (host allowlist), never the relay's name; errors from `lib/remoteErrors.ts` |
| Home | `pages/PagesOverview.tsx` (route kind `pages`/`home` in `Main.tsx`) | its shape is set by the `UX-HOME-*` rules in `docs/ux/spec.md` |
| Review lanes | `DistillReviewPanel.tsx` | pending/refinement flows |

## CONVENTIONS

- Preserve citation diagnosability. Existing tests cover verified/unverified
  chips, page-source ordering, missing/mismatched citation states, and popovers.
- Keep settings mutations going through the shared Tauri client wrappers.
- Treat `Main.tsx` changes as cross-view changes. Verify the affected view plus
  any query invalidation or keyboard shortcut behavior you touched.
- Prefer focused component tests near the edited file; many invariants here are
  easier to lock with Testing Library than with snapshots.

## ANTI-PATTERNS

- Do not hide unverified citations or collapse them into verified styling.
- Do not add navigation state that depends on localized placeholder text.
- Do not weaken `PageDetail.*.test.tsx` or `page/PageInfo.test.tsx` to make UI
  changes pass.
- Do not add a second home. Home is `pages/PagesOverview.tsx`; a library
  with no pages gets its empty state there, never a separate greeting screen.
  What home shows and leaves out is decided by the `UX-HOME-*` rules in
  `docs/ux/spec.md`; change the rule (Qi-Xuan's call) before the screen.
- Do not gate home copy on the `intelligence-ready` milestone. It is a
  permanent latch, so it keeps claiming a provider that has since been removed
  and denies one that was configured a minute ago. Read the live provider
  through `useProviderConfigured`.
- Ask settings IPC from the component whose copy branches on the answer, never
  higher. Home also renders in the fixture-only Review flavor, whose command
  contract (`review/commandCapabilities.ts`) fails closed on settings commands,
  so a hoisted read rejects on every Review home and breaks
  `e2e/review-flavor.review.spec.ts`.
- Do not put a retrieval list ("Where AI looked") on home. Agents reading from
  the library are the Activity view's subject (`ActivityFeed.tsx`, route kind
  `activity`); home stays the library's own surface.
- Do not promise pages will compile when no provider is configured. Page
  synthesis is LLM-gated, so that copy is a lie without a local model or an
  API key.

## COMMANDS

```bash
pnpm vitest run src/components/memory
pnpm vitest run src/components/memory/page
pnpm test:i18n
pnpm exec tsc -b
```
