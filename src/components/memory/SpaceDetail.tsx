// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useEffect, useRef, type ReactNode } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  confirmSpace,
  deleteSpace,
  getSpace,
  listPages,
  updateSpace,
  type Space,
  type IndexedFileInfo,
} from "../../lib/tauri";
import { SpaceProjectContent } from "./space-detail/SpaceProjectContent";
import { SpaceDossierHeader } from "./space-detail/SpaceDossierHeader";
import { RepoWikiButton } from "./space-detail/RepoWikiButton";
import { SPACE_DETAIL_KEY_COPY, type SpaceDetailCopy } from "./space-detail/copy";
import {
  PAGE_FETCH_LIMIT,
} from "./space-detail/model";
import { useUnobstructedContentWidth } from "./useUnobstructedContentWidth";
import "./space-detail/space-detail-header.css";
import "./space-detail/space-detail.css";
import "./pages/pageActions.css";

export type SpaceDetailProps = {
  readonly copy?: SpaceDetailCopy;
  readonly onBack: () => void;
  readonly onSpaceDeleted?: (spaceId: string) => void;
  readonly onSpaceLoaded?: (space: Space) => void;
  readonly onSpaceRenamed?: (space: Pick<Space, "id" | "name">) => void;
  readonly onReviewAll?: () => void;
  readonly onCreatePage?: (space: string) => void;
  readonly onSelectPage: (pageId: string) => void;
  readonly onOpenSource?: (file: IndexedFileInfo) => void;
  readonly onRegisterFlush?: (flush: (() => Promise<boolean>) | null) => void;
  readonly spaceName: string;
};

function SpaceDossierFrame({ children }: { readonly children: ReactNode }) {
  const frameRef = useUnobstructedContentWidth<HTMLElement>("--space-unobstructed-width", true);
  return <article ref={frameRef} className="space-dossier">{children}</article>;
}

export default function SpaceDetail({
  copy = SPACE_DETAIL_KEY_COPY,
  onBack,
  onSpaceDeleted,
  onSpaceLoaded,
  onSpaceRenamed,
  onReviewAll,
  onCreatePage,
  onSelectPage,
  onOpenSource,
  onRegisterFlush,
  spaceName,
}: SpaceDetailProps) {
  const queryClient = useQueryClient();
  const spaceQuery = useQuery({
    queryKey: ["space", spaceName],
    queryFn: () => getSpace(spaceName),
    refetchInterval: 5_000,
  });
  const pagesQuery = useQuery({
    queryKey: ["space-pages", spaceName],
    queryFn: () => listPages("active", spaceName, PAGE_FETCH_LIMIT),
    refetchInterval: 10_000,
  });
  const reportedSpaceIdRef = useRef<string | null>(null);
  const identityFlushRef = useRef<(() => Promise<boolean>) | null>(null);
  const previewFlushRef = useRef<(() => Promise<boolean>) | null>(null);
  const registerIdentityFlush = useCallback((flush: (() => Promise<boolean>) | null) => { identityFlushRef.current = flush; }, []);
  const registerPreviewFlush = useCallback((flush: (() => Promise<boolean>) | null) => { previewFlushRef.current = flush; }, []);
  const flushSpace = useCallback(async () => {
    if (identityFlushRef.current && !await identityFlushRef.current()) return false;
    if (previewFlushRef.current && !await previewFlushRef.current()) return false;
    return true;
  }, []);

  useEffect(() => {
    onRegisterFlush?.(flushSpace);
    return () => onRegisterFlush?.(null);
  }, [flushSpace, onRegisterFlush]);

  useEffect(() => {
    const space = spaceQuery.data;
    if (!space || reportedSpaceIdRef.current === space.id) return;
    reportedSpaceIdRef.current = space.id;
    onSpaceLoaded?.(space);
  }, [onSpaceLoaded, spaceQuery.data]);

  const invalidateSpaceQueries = () => {
    queryClient.invalidateQueries({ queryKey: ["space", spaceName] });
    queryClient.invalidateQueries({ queryKey: ["spaces"] });
    queryClient.invalidateQueries({ queryKey: ["space-memories", spaceName] });
    queryClient.invalidateQueries({ queryKey: ["space-entities", spaceName] });
    queryClient.invalidateQueries({ queryKey: ["space-pages", spaceName] });
    queryClient.invalidateQueries({ queryKey: ["memories"] });
  };

  const renameMutation = useMutation({
    mutationFn: ({ newName, description }: { readonly newName: string; readonly description?: string }) =>
      updateSpace(spaceName, newName, description),
    onSuccess: (updatedSpace, variables) => {
      // Seed the renamed route before it renders, keeping the inline draft mounted.
      queryClient.setQueryData(["space", spaceName], updatedSpace);
      queryClient.setQueryData(["space", updatedSpace.name], updatedSpace);
      invalidateSpaceQueries();
      if (variables.newName !== spaceName) {
        onSpaceRenamed?.({ id: updatedSpace.id, name: updatedSpace.name });
        if (!onSpaceRenamed) onBack();
      }
    },
  });
  const deleteMutation = useMutation({
    mutationFn: () => deleteSpace(spaceName),
    onSuccess: () => {
      invalidateSpaceQueries();
      if (spaceQuery.data && onSpaceDeleted) {
        // The owner replaces the deleted route directly. Running the ordinary
        // back flush afterward would target a Space that no longer exists.
        onSpaceDeleted(spaceQuery.data.id);
        return;
      }
      onBack();
    },
  });
  const confirmMutation = useMutation({
    mutationFn: () => confirmSpace(spaceName),
    onSuccess: invalidateSpaceQueries,
  });

  if (spaceQuery.isPending) {
    return <p className="space-dossier-state" role="status">{copy.loading}</p>;
  }
  if (spaceQuery.isError) {
    return <p className="space-dossier-state" role="alert">{copy.loadError}</p>;
  }
  if (!spaceQuery.data) {
    return <p className="space-dossier-state">{copy.notFound}</p>;
  }

  const space = spaceQuery.data;
  const pages = pagesQuery.data ?? [];
  const mutationError = renameMutation.isError || deleteMutation.isError || confirmMutation.isError;

  return (
    <SpaceDossierFrame>
      <SpaceDossierHeader
        key={space.id}
        leadingAction={<RepoWikiButton space={space.name} />}
        onRegisterFlush={registerIdentityFlush}
        actions={{
          onBack,
          onDelete: async () => {
            const flushPreview = previewFlushRef.current;
            try {
              if (flushPreview && !await flushPreview()) return;
            } catch {
              return;
            }
            deleteMutation.mutate();
          },
          onKeep: () => confirmMutation.mutate(),
          onReviewAll,
          onCreatePage: () => onCreatePage?.(space.name),
          onSaveIdentity: async ({ name, description }) => {
            if (name !== space.name && previewFlushRef.current && !await previewFlushRef.current()) {
              throw new Error("The source description could not be saved.");
            }
            await renameMutation.mutateAsync({ newName: name, description });
          },
        }}
        copy={copy}
        mutationError={mutationError}
        space={space}
      />

      <SpaceProjectContent
        key={`${space.id}:${space.name}`}
        copy={copy}
        spaceId={space.id}
        spaceName={space.name}
        pages={pages}
        pagesPending={pagesQuery.isPending}
        pagesError={pagesQuery.isError}
        onRetryPages={() => void pagesQuery.refetch()}
        onSelectPage={onSelectPage}
        onOpenSource={onOpenSource}
        onRegisterPreviewFlush={registerPreviewFlush}
      />
    </SpaceDossierFrame>
  );
}
