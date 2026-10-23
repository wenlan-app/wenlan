// SPDX-License-Identifier: AGPL-3.0-only
import type {
  ArchiveEntitiesRequest,
  KnowledgeFolder,
  DistillReviewResponse,
  Entity,
  EntityBulkResponse,
  EntityDetail,
  ListEntitiesRequest,
  ListEntitiesResponse,
  MemoryItem,
  IngestWebpageRequest,
  Page as KnowledgePage,
  PageRedistillResponse,
  PageReviewOutcome,
  CodeBasisLocator,
  RefinementProposalSummary,
  RestoreEntitiesRequest,
  SourceAnnotation,
  Space,
  UpdatePageInput,
  UpdatePageOutcome,
  UpdateSourceAnnotationRequest,
} from "../../src/lib/tauri";
import { daemonMeetsFloor } from "../../src/lib/daemonVersion";
import type { SpacesNavigationFixture } from "../fixtures/spacesNavigation";
import { baseResponse } from "./baseResponses";
import { ConfiguredTauriFailureError, TauriMockArgumentError } from "./errors";
import { PageBasisStore } from "./pageBasis";
import { PageCodeBasisStore } from "./pageCodeBasis";
import { PageMapStore } from "./pageMap";
import type { MockCommandCall, MockFailure, TauriMockPageScenario } from "./types";

const CLIENT_PAGE_DRAFT_ID =
  /^page_[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
const SOURCES_BLOCK_START = "<!-- origin:sources:start -->";
const SOURCES_BLOCK_END = "<!-- origin:sources:end -->";
const PAGE_EDIT_DAEMON_FLOOR = "0.14.1";

type PageDraftCreateRequest = {
  readonly title: string;
  readonly content: string;
  readonly space: string | null;
};

function canonicalizePageBody(body: string): string {
  const start = body.indexOf(SOURCES_BLOCK_START);
  if (start < 0) return body.trimEnd();
  const afterStart = start + SOURCES_BLOCK_START.length;
  const relativeEnd = body.slice(afterStart).indexOf(SOURCES_BLOCK_END);
  if (relativeEnd < 0) return body.trimEnd();
  const end = afterStart + relativeEnd + SOURCES_BLOCK_END.length;
  const head = body.slice(0, start).trimEnd();
  const tail = body.slice(end).trimStart();
  return (tail ? `${head}\n\n${tail}` : head).trimEnd();
}

function sanitizeIngressContent(content: string): string {
  return canonicalizePageBody(content)
    .replaceAll(SOURCES_BLOCK_START, "")
    .replaceAll(SOURCES_BLOCK_END, "")
    .trimEnd();
}

function generatedDraftTitle(content: string): string {
  for (const line of content.split(/\r?\n/u)) {
    let title = line.replace(/[\u0000-\u001f\u007f-\u009f]/gu, " ")
      .replace(/^\p{White_Space}+|\p{White_Space}+$/gu, "");
    const heading = /^(#{1,6})(?=\p{White_Space})/u.exec(title);
    if (heading) title = title.slice(heading[0].length);
    title = title.split(/\p{White_Space}+/u).filter(Boolean).join(" ");
    if (title) return Array.from(title).slice(0, 80).join("");
  }
  return "Untitled page";
}

function nextFixtureTimestamp(previous: string | null | undefined): string {
  const floor = Date.parse("2026-07-10T12:34:00Z");
  const previousMs = typeof previous === "string" ? Date.parse(previous) : Number.NaN;
  return new Date(Math.max(
    floor,
    Number.isFinite(previousMs) ? previousMs + 1 : 0,
  )).toISOString();
}

function optionalValue(args: unknown, key: string): unknown {
  return typeof args === "object" && args !== null ? Reflect.get(args, key) : undefined;
}

function requiredString(command: string, args: unknown, key: string): string {
  const value = optionalValue(args, key);
  if (typeof value !== "string" || value.trim() === "") throw new TauriMockArgumentError(command, key);
  return value;
}

function stringValue(command: string, args: unknown, key: string): string {
  const value = optionalValue(args, key);
  if (typeof value !== "string") throw new TauriMockArgumentError(command, key);
  return value;
}

function optionalString(args: unknown, key: string): string | null {
  const value = optionalValue(args, key);
  return typeof value === "string" ? value : null;
}

function requiredNumber(command: string, args: unknown, key: string): number {
  const value = optionalValue(args, key);
  if (typeof value !== "number" || !Number.isFinite(value)) throw new TauriMockArgumentError(command, key);
  return value;
}

function requiredBoolean(command: string, args: unknown, key: string): boolean {
  const value = optionalValue(args, key);
  if (typeof value !== "boolean") throw new TauriMockArgumentError(command, key);
  return value;
}

/** The `locator` argument of a code basis command: the four fields that name a row. */
function locatorArg(command: string, args: unknown): CodeBasisLocator {
  const locator = optionalValue(args, "locator");
  return {
    repo_root: requiredString(command, locator, "repo_root"),
    path: requiredString(command, locator, "path"),
    line_start: requiredNumber(command, locator, "line_start"),
    line_end: requiredNumber(command, locator, "line_end"),
  };
}

function stringArray(args: unknown, key: string): readonly string[] {
  const value = optionalValue(args, key);
  return Array.isArray(value) ? value.filter((item): item is string => typeof item === "string") : [];
}

export class TauriMockRuntime {
  private spaces: Space[];
  private folders: KnowledgeFolder[];
  private readonly folderMoveReceipts = new Map<string, {id:string;expectedStoragePath:string;folderPath:string;storage_path:string}>();
  private pages: KnowledgePage[];
  private readonly sourceAnnotations = new Map<string, SourceAnnotation>();
  private entityDetails: EntityDetail[];
  private observationSequence = 0;
  private readonly observationIds = new Set<string>();
  private memories: MemoryItem[];
  private readonly webpages = new Map<string, IngestWebpageRequest>();
  private readonly distillReview: DistillReviewResponse;
  private refinements: RefinementProposalSummary[];
  private readonly callsLog: MockCommandCall[] = [];
  private readonly failures = new Map<string, string[]>();
  private readonly activityRows: readonly Record<string, unknown>[];
  private readonly pageDraftCreateRequests = new Map<string, PageDraftCreateRequest | null>();
  private readonly pageMaps = new PageMapStore();
  private readonly pageBasis: PageBasisStore;
  private readonly pageCodeBasis: PageCodeBasisStore;
  private readonly updatePageReceipts = new Map<
    string,
    { readonly input: UpdatePageInput; readonly outcome: UpdatePageOutcome }
  >();
  private firstWriteRemoteMutationPending: boolean;
  private pageSequence: number;
  private backgroundAiEnabled = false;
  private lockMyPages = false;
  private projectionFailureIssued = false;

  constructor(
    private readonly fixture: SpacesNavigationFixture,
    failures: readonly MockFailure[] = [],
    rawActions: readonly string[] = [],
    private readonly pageScenario: TauriMockPageScenario = {},
    private readonly delays: Readonly<Record<string, number>> = {},
  ) {
    this.folders = (fixture.folders ?? []).map(folder => ({...folder}));
    this.spaces = fixture.spaces.map((space) => ({ ...space }));
    this.pages = fixture.pages.map((page) => ({ ...page }));
    this.pageSequence = this.pages.length;
    this.pageBasis = new PageBasisStore(pageScenario.basis);
    this.pageCodeBasis = new PageCodeBasisStore(pageScenario.codeBasis);
    this.entityDetails = fixture.entityDetails.map((detail) => ({
      entity: { ...detail.entity },
      observations: detail.observations.map((observation) => ({ ...observation })),
      relations: detail.relations.map((relation) => ({ ...relation })),
    }));
    for (const detail of this.entityDetails) {
      for (const observation of detail.observations) this.observationIds.add(observation.id);
    }
    this.memories = fixture.memories.map((memory) => ({ ...memory }));
    this.distillReview = structuredClone(fixture.distillReview);
    this.refinements = structuredClone([...fixture.refinements]);
    this.firstWriteRemoteMutationPending =
      pageScenario.firstWriteRemoteMutation !== undefined;
    this.activityRows = rawActions.map((action, index) => ({
      id: index + 1,
      timestamp: 1_783_728_000 - index * 60,
      agent_name: "claude-code",
      action,
      memory_ids: null,
      query: null,
      detail: null,
      memory_titles: [],
    }));
    for (const failure of failures) this.failNext(failure.command, failure.message, failure.times ?? 1);
  }

  calls(): readonly MockCommandCall[] {
    return this.callsLog.map((call) => ({ ...call }));
  }

  failNext(command: string, message: string, times = 1): void {
    const queued = this.failures.get(command) ?? [];
    this.failures.set(command, [...queued, ...Array.from({ length: times }, () => message)]);
  }

  async invoke(command: string, args?: unknown): Promise<unknown> {
    this.callsLog.push({ command, args });
    const configured = this.failures.get(command)?.shift();
    if (configured) throw new ConfiguredTauriFailureError(command, configured);
    if (command.startsWith("plugin:")) return null;
    if (command === "get_background_ai_enabled") return this.backgroundAiEnabled;
    if (command === "set_background_ai_enabled") {
      this.backgroundAiEnabled = requiredBoolean(command, args, "enabled");
      return null;
    }
    if (command === "get_lock_my_pages") return this.lockMyPages;
    if (command === "set_lock_my_pages") {
      this.lockMyPages = requiredBoolean(command, args, "enabled");
      if (!this.lockMyPages) return null;
      // Like the daemon: lock every unlocked page a person wrote or edited.
      let locked = 0;
      this.pages = this.pages.map((page) => {
        if (!page.user_edited || page.review_required || page.creation_kind === "entity") return page;
        locked += 1;
        return { ...page, review_required: true };
      });
      return locked;
    }
    if (command === "get_activity") {
      const activity = baseResponse(command, args, { activityRows: this.activityRows, memoryCount: this.memories.length }) as Record<string, unknown>;
      return { ...activity, state: this.backgroundAiEnabled ? activity.state : "off" };
    }
    const delay = this.delays[command];
    if (delay) await new Promise((resolve) => setTimeout(resolve, delay));

    switch (command) {
      case "knowledge_folders_list": return { folders: structuredClone(this.folders), truncated: false };
      case "knowledge_folder_create": {
        const parentPath = stringValue(command, args, "parentPath");
        const name = requiredString(command, args, "name");
        if (name.startsWith(".") || /[\\/:]/.test(name) || (parentPath && !this.folders.some(folder => folder.path === parentPath))) throw new Error("Invalid folder");
        const path = parentPath ? `${parentPath}/${name}` : name;
        if (this.folders.some(folder => folder.path === path)) throw new Error("Folder already exists");
        this.folders.push({path,parent_path:parentPath,name}); return {path};
      }
      case "page_move": {
        const id = requiredString(command, args, "id");
        const expectedStoragePath = requiredString(command, args, "expectedStoragePath");
        const folderPath = stringValue(command, args, "folderPath");
        const operationId = requiredString(command, args, "operationId");
        const receipt = this.folderMoveReceipts.get(operationId);
        if (receipt) {
          if (receipt.id !== id || receipt.expectedStoragePath !== expectedStoragePath || receipt.folderPath !== folderPath) throw new Error("Move operation conflict");
          return {storage_path:receipt.storage_path};
        }
        const page = this.pages.find(note => note.id === id);
        if (!page || page.storage_path !== expectedStoragePath) throw new Error("Storage path changed");
        if (folderPath && !this.folders.some(folder => folder.path === folderPath)) throw new Error("Folder missing");
        const filename = expectedStoragePath.split("/").slice(-1)[0];
        const storage_path = folderPath ? `${folderPath}/${filename}` : filename;
        if (this.pages.some(note => note.id !== id && note.storage_path === storage_path)) throw new Error("Destination exists");
        this.pages = this.pages.map(note => note.id === id ? { ...note, storage_path } : note);
        this.folderMoveReceipts.set(operationId,{id,expectedStoragePath,folderPath,storage_path});
        return {storage_path};
      }
      case "repair_resume_runtime": return undefined;
      case "repair_operation_status":
      case "repair_cancel":
      case "repair_prepare_operation":
      case "repair_prepare_operation_status":
      case "repair_prepare_operation_cancel":
        throw new Error("Source repair operation status requires native durable artifacts.");
      case "repair_recovery": {
        requiredString(command, args, "reviewId");
        // This fixture has no durable native repair artifacts.
        return null;
      }
      case "list_spaces": return this.spaces.map((space) => ({ ...space }));
      case "get_space": return this.spaces.find((space) => space.name === requiredString(command, args, "name")) ?? null;
      case "create_space": return this.createSpace(args);
      case "update_space": return this.updateSpace(args);
      case "delete_space": return this.deleteSpace(args);
      case "confirm_space": return this.confirmSpace(args);
      case "toggle_space_starred": return this.toggleSpace(args);
      case "reorder_space": return this.reorderSpace(args);
      case "daemon_version": return this.pageScenario.daemonVersion ?? "0.14.1";
      case "get_system_info": return {
        total_ram_gb: 32,
        available_ram_gb: 16,
        has_metal: true,
        has_cuda: false,
        os: "macos",
        arch: "aarch64",
        recommended_builtin: "",
      };
      case "record_page_editor_diagnostic": return null;
      case "list_pages": return this.listPages(args);
      case "list_recent_changes": return this.pageScenario.recentChanges ?? [];
      case "get_page_links": {
        const id = requiredString(command, args, "pageId");
        return {
          outbound: (this.pageScenario.outboundLinks?.[id] ?? []).map((link) => ({
            ...link,
            target_title: this.pages.find((target) => target.id === link.target_page_id)?.title ?? null,
          })),
          inbound: [],
        };
      }
      case "set_page_review_required": {
        const pageId = requiredString(command, args, "pageId");
        const required = requiredBoolean(command, args, "required");
        const index = this.pages.findIndex((entry) => entry.id === pageId);
        const page = this.pages[index];
        // The daemon never matches an entity shadow page.
        if (index < 0 || !page || page.creation_kind === "entity") throw new TauriMockArgumentError(command, "pageId");
        // The daemon omits the field while false; keep that shape.
        const { review_required: _previous, ...rest } = page;
        this.pages[index] = required ? { ...rest, review_required: true } : rest;
        return { page_id: pageId, review_required: required };
      }
      case "mark_page_basis":
        return this.pageBasis.mark(
          requiredString(command, args, "pageId"),
          requiredString(command, args, "basisPageId"),
          this.pages,
        );
      case "unmark_page_basis":
        return this.pageBasis.unmark(
          requiredString(command, args, "pageId"),
          requiredString(command, args, "basisPageId"),
        );
      case "confirm_page_basis":
        return this.pageBasis.confirm(
          requiredString(command, args, "pageId"),
          requiredString(command, args, "basisPageId"),
          requiredNumber(command, args, "version"),
          this.pages,
        );
      case "get_page_basis_diff":
        return this.pageBasis.diff(
          requiredString(command, args, "pageId"),
          requiredString(command, args, "basisPageId"),
          this.pages,
        );
      case "list_basis_pending":
        return this.pageBasis.pending(optionalString(args, "space"), this.pages);
      case "list_code_basis_pending":
        return this.pageCodeBasis.pending(optionalString(args, "space"), this.pages);
      case "confirm_page_code_basis":
        return this.pageCodeBasis.confirm(requiredString(command, args, "pageId"), locatorArg(command, args));
      case "remove_page_code_basis":
        return this.pageCodeBasis.remove(requiredString(command, args, "pageId"), locatorArg(command, args));
      case "get_page_code_basis_cited":
        return this.pageCodeBasis.cited(requiredString(command, args, "pageId"), locatorArg(command, args));
      case "read_git_blob_lines":
        return this.pageCodeBasis.readBlobLines(
          requiredString(command, args, "blob"),
          requiredNumber(command, args, "lineStart"),
          requiredNumber(command, args, "lineEnd"),
        );
      case "read_code_lines":
        return this.pageCodeBasis.readLines(
          requiredString(command, args, "path"),
          requiredNumber(command, args, "lineStart"),
          requiredNumber(command, args, "lineEnd"),
        );
      case "get_page_sources": {
        const pageId = requiredString(command, args, "pageId");
        const page = this.pages.find((entry) => entry.id === pageId);
        if (!page) throw new TauriMockArgumentError(command, "pageId");
        return page.source_memory_ids.map((id) => ({
          source: { page_id: pageId, memory_source_id: id, linked_at: 1_783_728_000 },
          memory: this.memories.find((memory) => memory.source_id === id) ?? null,
        }));
      }
      case "get_source_annotation": return this.getSourceAnnotation(args);
      case "update_source_annotation": return this.updateSourceAnnotation(args);
      case "get_page": return this.getPageResponse(command, args);
      // Explicit-browse variants attach a human-intent header at the HTTP
      // layer; this fixture has no HTTP layer, so they resolve identically
      // to their automatic counterparts.
      case "list_pages_explicit_browse": return this.listPages(args);
      case "get_page_explicit_browse": return this.getPageResponse(command, args);
      // No fixture models a live M5 truth cutover yet, so this always
      // reports "not cut over" — the same state as production before cutover.
      case "get_truth_status": return null;
      // Review is a fixture surface; no import is ever in flight in it, so
      // the home screen's poll answers "nothing settling" rather than being
      // rejected as an unknown command.
      case "active_import_batches_cmd": return { batches: [] };
      case "create_page": return this.createPage(args);
      case "rename_page": return this.renamePage(args);
      case "update_page": return this.updatePage(args);
      case "delete_page": return this.deletePage(args);
      case "redistill_page": return this.redistillPage(args);
      case "page_review_supported": return this.pageScenario.pageReview?.availability ?? "ready";
      case "review_page": return this.reviewPage(args);
      case "get_page_map": return this.pageMaps.get(args);
      case "improve_page_map": return this.pageMaps.improve(args);
      case "create_page_map_node": return this.pageMaps.create(args);
      case "patch_page_map_node": return this.pageMaps.patch(args);
      case "delete_page_map_node": return this.pageMaps.remove(args);
      case "put_page_map_layout": return this.pageMaps.layout(args);
      case "create_page_draft": return this.createPageDraft(args);
      case "update_page_draft": return this.updatePageDraft(args);
      case "publish_page_draft": return this.publishPageDraft(args);
      case "discard_page_draft": return this.discardPageDraft(args);
      case "distill_review": return structuredClone(this.distillReview);
      // The fixture library has no material waiting for pages.
      case "list_new_material": return { items: [], total: 0 };
      case "list_pending_revisions": return structuredClone(this.fixture.pendingRevisions ?? []);
      case "list_refinements": return { proposals: structuredClone(this.refinements) };
      case "accept_refinement": return this.resolveRefinement(args, true);
      case "reject_refinement": return this.resolveRefinement(args, false);
      case "list_entities_cmd": return this.listEntities(args);
      case "get_entity_detail_cmd": return this.getEntityDetail(args);
      case "get_knowledge_graph_cmd": return this.getKnowledgeGraph();
      case "add_observation_cmd": return this.addObservation(args);
      case "update_observation_cmd": return this.updateObservation(args);
      case "delete_observation_cmd": return this.deleteObservation(args);
      case "confirm_observation_cmd": return this.confirmObservation(args);
      case "confirm_entity_cmd": return this.confirmEntity(args);
      case "change_entity_type_cmd": return this.changeEntityType(args);
      case "delete_entity_cmd": return this.deleteEntity(args);
      case "query_entities_cmd": return this.queryEntities(args);
      case "archive_entities_cmd": return this.archiveEntities(args);
      case "restore_entities_cmd": return this.restoreEntities(args);
      case "list_memories_cmd": return this.listMemories(args);
      case "get_memory_detail": return this.memories.find((memory) => memory.source_id === requiredString(command, args, "sourceId")) ?? null;
      case "list_memories_by_ids": {
        const ids = new Set(stringArray(args, "ids"));
        return this.memories.filter((memory) => ids.has(memory.source_id));
      }
      case "list_indexed_files": return this.listIndexedFiles().filter(file => !optionalString(args, "space") || file.space === optionalString(args, "space") || file.domain === optionalString(args, "space"));
      case "ingest_webpage": return this.ingestWebpage(args);
      case "get_chunks": {
        const scope = optionalString(args, "space");
        const document = this.fixture.documents?.find(({ file }) => file.source === optionalString(args, "source") && file.source_id === optionalString(args, "sourceId") && (!scope || file.space === scope || file.domain === scope));
        if (document) return [{ id: `${document.file.source}:${document.file.source_id}`, content: document.content, chunk_index: 0, chunk_type: "text", language: null }];
        if (scope && this.fixture.documents) return [];
        if (optionalString(args, "source") !== "webpage") return baseResponse(command, args, { activityRows: this.activityRows, memoryCount: this.memories.length });
        const sourceId = requiredString(command, args, "sourceId");
        const webpage = this.webpages.get(sourceId);
        return webpage ? [{ id: `webpage-${sourceId}`, content: webpage.content, chunk_index: 0, chunk_type: "text", language: null }] : [];
      }
      case "update_memory_cmd": return this.updateMemory(args);
      case "reclassify_memory_cmd": return this.reclassifyMemory(args);
      case "set_stability_cmd": return this.setStability(args);
      case "delete_file_chunks": return this.deleteFileChunks(args);
      case "pin_memory": return this.setMemoryPinned(args, true);
      case "unpin_memory": return this.setMemoryPinned(args, false);
      case "confirm_memory": return this.confirmMemory(args);
      case "delete_memory": return this.deleteMemory(args);
      case "acknowledge_onboarding_milestone":
        requiredString(command, args, "id");
        return null;
      case "search": return this.search(args);
      default: return baseResponse(command, args, { activityRows: this.activityRows, memoryCount: this.memories.length });
    }
  }

  private createSpace(args: unknown): Space {
    const name = requiredString("create_space", args, "name");
    const space: Space = {
      id: `space-${name.toLowerCase().replaceAll(/[^a-z0-9]+/g, "-")}`,
      name, description: optionalString(args, "description"), suggested: false, starred: false,
      sort_order: this.spaces.length, memory_count: 0, entity_count: 0,
      created_at: 1_783_728_000, updated_at: 1_783_728_000,
    };
    this.spaces.push(space);
    return { ...space };
  }

  private updateSpace(args: unknown): Space {
    const name = requiredString("update_space", args, "name");
    const newName = requiredString("update_space", args, "newName");
    const index = this.spaces.findIndex((space) => space.name === name);
    if (index < 0) throw new TauriMockArgumentError("update_space", "name");
    const current = this.spaces[index];
    if (!current) throw new TauriMockArgumentError("update_space", "name");
    if (
      name !== newName
      && this.spaces.some((space, candidateIndex) =>
        candidateIndex !== index && space.name === newName
      )
    ) {
      throw new Error(`Space ${JSON.stringify(newName)} already exists`);
    }
    const updated = { ...current, name: newName, description: optionalString(args, "description") };
    if (name !== newName) {
      this.pages = this.pages.map((page) => {
        if (page.space !== name && page.domain !== name) return page;
        return {
          ...page,
          space: page.space === name ? newName : page.space,
          domain: page.domain === name ? newName : page.domain,
          version: page.status === "draft" ? page.version + 1 : page.version,
          last_modified: page.status === "draft"
            ? nextFixtureTimestamp(page.last_modified)
            : page.last_modified,
        };
      });
    }
    this.spaces[index] = updated;
    return { ...updated };
  }

  private deleteSpace(args: unknown): null {
    const name = requiredString("delete_space", args, "name");
    this.spaces = this.spaces.filter((space) => space.name !== name);
    return null;
  }

  private confirmSpace(args: unknown): null {
    const name = requiredString("confirm_space", args, "name");
    this.spaces = this.spaces.map((space) => space.name === name ? { ...space, suggested: false } : space);
    return null;
  }

  private toggleSpace(args: unknown): boolean {
    const name = requiredString("toggle_space_starred", args, "name");
    let starred = false;
    this.spaces = this.spaces.map((space) => {
      if (space.name !== name) return space;
      starred = !space.starred;
      return { ...space, starred };
    });
    return starred;
  }

  private reorderSpace(args: unknown): null {
    const name = requiredString("reorder_space", args, "name");
    const newOrder = requiredNumber("reorder_space", args, "newOrder");
    // Mirror the daemon's transactional range shift, not just the moved rank.
    const source = this.spaces.find((space) => space.name === name);
    if (!source || source.sort_order === newOrder) return null;
    const oldOrder = source.sort_order;
    this.spaces = this.spaces.map((space) => {
      if (space.name === name) return { ...space, sort_order: newOrder };
      if (newOrder < oldOrder && space.sort_order >= newOrder && space.sort_order < oldOrder) {
        return { ...space, sort_order: space.sort_order + 1 };
      }
      if (newOrder > oldOrder && space.sort_order > oldOrder && space.sort_order <= newOrder) {
        return { ...space, sort_order: space.sort_order - 1 };
      }
      return space;
    });
    return null;
  }

  private getPageResponse(command: string, args: unknown): KnowledgePage | null {
    const page = structuredClone(this.pages.find(page => page.id === requiredString(command, args, "id")) ?? null);
    if (page) {
      // Like the real GET contract: a publish outcome is session information,
      // not a durable error inferred from a missing file on later reads.
      delete page.projection_status;
      delete page.projection_error;
      // Like the real GET: the bases ride with the note.
      const basis = this.pageBasis.refsFor(page.id, this.pages);
      if (basis.length > 0) page.basis = basis;
      const codeBasis = this.pageCodeBasis.refsFor(page.id);
      if (codeBasis.length > 0) page.code_basis = codeBasis;
    }
    return page;
  }

  private listPages(args: unknown): readonly KnowledgePage[] {
    const status = optionalString(args, "status");
    const domain = optionalString(args, "domain");
    const limit = optionalValue(args, "limit");
    const offset = optionalValue(args, "offset");
    const filtered = this.pages.filter((page) => (!status || page.status === status) && (!domain || page.domain === domain || page.space === domain));
    return structuredClone(filtered.slice(typeof offset === "number" ? offset : 0, typeof limit === "number" ? (typeof offset === "number" ? offset : 0) + limit : undefined));
  }

  private createPage(args: unknown): { id: string; attached_to: string | null; warnings: string[] } {
    const title = requiredString("create_page", args, "title").trim();
    const content = requiredString("create_page", args, "content").trim();
    const space = optionalString(args, "space")?.trim() || null;
    if (space && !this.spaces.some((candidate) => candidate.name === space)) {
      throw new TauriMockArgumentError("create_page", "space");
    }
    const id = `page-authored-${++this.pageSequence}`;
    const now = "2026-07-10T12:30:00Z";
    this.pages = [{
      id,
      title,
      summary: null,
      content,
      entity_id: null,
      domain: space,
      space,
      source_memory_ids: [],
      version: 1,
      status: "active",
      creation_kind: "authored",
      review_status: "unconfirmed",
      created_at: now,
      last_compiled: now,
      last_modified: now,
    }, ...this.pages];
    return { id, attached_to: null, warnings: [] };
  }

  private deletePage(args: unknown): null {
    const id = requiredString("delete_page", args, "id");
    this.pages = this.pages.filter((page) => page.id !== id);
    return null;
  }

  private updatePage(args: unknown): UpdatePageOutcome | null {
    const id = requiredString("update_page", args, "id");
    const content = requiredString("update_page", args, "content");
    if (optionalValue(args, "expectedVersion") === undefined) {
      const legacyIndex = this.pages.findIndex((page) => page.id === id);
      const legacyPage = this.pages[legacyIndex];
      if (legacyIndex < 0 || !legacyPage) {
        throw new TauriMockArgumentError("update_page", "id");
      }
      this.pages[legacyIndex] = {
        ...legacyPage,
        content,
        citations: [],
        user_edited: true,
        version: legacyPage.version + 1,
        last_modified: "2026-07-10T12:32:00Z",
      };
      return null;
    }
    const expectedVersion = requiredNumber(
      "update_page",
      args,
      "expectedVersion",
    );
    const callerId = requiredString("update_page", args, "callerId");
    const operationId = requiredString("update_page", args, "operationId");
    if (callerId !== "wenlan-app") {
      return {
        outcome: "failure",
        kind: "auth_required",
        status: 403,
        message: "caller is not authorized to update pages",
      };
    }
    const reportedVersion =
      this.pageScenario.saveDaemonVersion
      ?? this.pageScenario.daemonVersion
      ?? PAGE_EDIT_DAEMON_FLOOR;
    if (!daemonMeetsFloor(reportedVersion, PAGE_EDIT_DAEMON_FLOOR)) {
      return {
        outcome: "upgrade_required",
        reportedVersion,
        requiredFloor: PAGE_EDIT_DAEMON_FLOOR,
      };
    }
    const input: UpdatePageInput = {
      id,
      content,
      expectedVersion,
      callerId,
      operationId,
    };
    const receiptKey = `${callerId}\u0000${operationId}`;
    const receipt = this.updatePageReceipts.get(receiptKey);
    if (receipt) {
      const sameRequest =
        receipt.input.id === input.id
        && receipt.input.content === input.content
        && receipt.input.expectedVersion === input.expectedVersion;
      return sameRequest
        ? structuredClone(receipt.outcome)
        : {
            outcome: "conflict",
            message:
              "operation identity was already used for different page content",
          };
    }

    if (this.firstWriteRemoteMutationPending) {
      this.firstWriteRemoteMutationPending = false;
      const mutation = this.pageScenario.firstWriteRemoteMutation;
      if (mutation) {
        const remoteIndex = this.pages.findIndex(
          (page) => page.id === mutation.pageId,
        );
        const remotePage = this.pages[remoteIndex];
        if (remoteIndex >= 0 && remotePage) {
          this.pages[remoteIndex] = {
            ...remotePage,
            content: mutation.content,
            user_edited: true,
            version: remotePage.version + 1,
            last_modified: "2026-07-10T12:31:00Z",
          };
        }
      }
    }

    const index = this.pages.findIndex((page) => page.id === id);
    const page = this.pages[index];
    if (index < 0 || !page) {
      return {
        outcome: "failure",
        kind: "not_found",
        status: 404,
        message: "page not found",
      };
    }
    if (page.version !== expectedVersion) {
      return {
        outcome: "conflict",
        message:
          `expected version ${expectedVersion}; current version is ${page.version}`,
      };
    }
    this.pages[index] = {
      ...page,
      content,
      citations: [],
      user_edited: true,
      version: page.version + 1,
      last_modified: "2026-07-10T12:32:00Z",
    };
    const confirmBasis = optionalValue(args, "confirmBasis");
    const basisConfirm =
      typeof confirmBasis === "object" && confirmBasis !== null
        ? this.pageBasis.confirmFromSave(
            id,
            confirmBasis as { basis_page_id: string; version: number },
            this.pages,
          )
        : undefined;
    const outcome: UpdatePageOutcome = basisConfirm
      ? { outcome: "saved", basisConfirm }
      : { outcome: "saved" };
    this.updatePageReceipts.set(receiptKey, {
      input: structuredClone(input),
      outcome: structuredClone(outcome),
    });
    return outcome;
  }

  private renamePage(args: unknown): { id: string; title: string; version: number } {
    const id = requiredString("rename_page", args, "id");
    const title = requiredString("rename_page", args, "title").trim();
    const expectedVersion = requiredNumber("rename_page", args, "expectedVersion");
    const index = this.pages.findIndex((page) => page.id === id);
    const current = this.pages[index];
    if (!current) throw new Error("page not found (404)");
    if (current.status !== "active" || current.creation_kind === "entity" || current.creation_kind === "source") {
      throw new Error("page cannot be renamed (403)");
    }
    if (current.version !== expectedVersion) throw new Error("page version conflict (409)");
    if (this.pages.some((page) => page.id !== id && page.title.toLocaleLowerCase() === title.toLocaleLowerCase())) {
      throw new Error("page title collision (409)");
    }
    const renamed = { ...current, title, version: current.version + 1 };
    this.pages[index] = renamed;
    return { id: renamed.id, title: renamed.title, version: renamed.version };
  }

  private redistillPage(args: unknown): PageRedistillResponse {
    const id = requiredString("redistill_page", args, "pageId");
    const index = this.pages.findIndex((page) => page.id === id);
    const page = this.pages[index];
    if (index < 0 || !page) throw new TauriMockArgumentError("redistill_page", "pageId");
    // A note that relies on other notes never takes the rebuild directly: the
    // daemon stages a preview card, or finds the note already says it all.
    const preview = this.pageBasis.redistillOutcome(id);
    if (preview === "unchanged") return { status: "ok", updated: false, unchanged: true };
    if (preview) {
      return { status: "ok", updated: false, gated: true, revision_card_id: preview.gated };
    }
    this.pages[index] = {
      ...page,
      last_compiled: "2026-07-10T12:33:00Z",
      last_modified: "2026-07-10T12:33:00Z",
      stale_reason: null,
    };
    return { status: "ok", updated: true };
  }

  /**
   * Answers `review_page` the way the daemon does, including the one refusal
   * a fixture can actually provoke.
   *
   * The daemon binds a review to a digest of the exact text the reader had on
   * screen and refuses with `presence_conflict` when the stored page has moved
   * on since (threat model T6). Comparing content here is the same check, and
   * it is what lets a spec drive the stale path by editing the page out from
   * under the reader rather than by asking the mock to lie. Refusals a fixture
   * cannot reach on its own — an unreadable install secret, a MAC that does
   * not verify — are set through `pageScenario.pageReview.outcome` instead.
   */
  private reviewPage(args: unknown): PageReviewOutcome {
    const id = requiredString("review_page", args, "pageId");
    const content = stringValue("review_page", args, "content");
    const forced = this.pageScenario.pageReview?.outcome;
    if (forced) return forced;
    const page = this.pages.find((candidate) => candidate.id === id);
    if (!page) return { kind: "invalid" };
    if (page.content !== content) return { kind: "stale" };
    return { kind: "applied", reviewed_page_version: page.version };
  }

  private resolveRefinement(
    args: unknown,
    approve: boolean,
  ): { id: string; action_applied: string } | { id: string } {
    const command = approve ? "accept_refinement" : "reject_refinement";
    const id = requiredString(command, args, "id");
    const index = this.refinements.findIndex((proposal) => proposal.id === id);
    const proposal = this.refinements[index];
    if (index < 0 || !proposal) throw new TauriMockArgumentError(command, "id");
    this.refinements.splice(index, 1);
    if (approve && proposal.payload?.action === "page_keep_or_archive") {
      const pageId = proposal.source_ids[0];
      this.pages = this.pages.map((page) =>
        page.id === pageId ? { ...page, status: "archived" } : page
      );
    }
    if (approve && proposal.payload?.action === "vocab_promote" && proposal.payload.kind === "entity") {
      const canonical = proposal.payload.old_value.toLowerCase();
      this.entityDetails = this.entityDetails.map((detail) =>
        proposal.source_ids.includes(detail.entity.id) && detail.entity.entity_type === "concept" && detail.entity.status !== "archived"
          ? { ...detail, entity: { ...detail.entity, entity_type: canonical } }
          : detail
      );
    }
    return approve ? { id, action_applied: proposal.action } : { id };
  }

  private createPageDraft(args: unknown): KnowledgePage {
    const id = requiredString("create_page_draft", args, "clientDraftId");
    if (!CLIENT_PAGE_DRAFT_ID.test(id)) {
      throw new Error("Page draft id must use the page_<uuid-v4> format");
    }
    const title = stringValue("create_page_draft", args, "title");
    const content = stringValue("create_page_draft", args, "content");
    if (!title.trim() && !sanitizeIngressContent(content).trim()) {
      throw new Error(JSON.stringify({
        code: "invalid_page_draft",
        error: "A Page draft needs a title or content",
      }));
    }
    const requestedSpace = this.normalizedDraftSpace(args);
    const existing = this.pages.find((page) => page.id === id);
    if (existing) {
      const request = this.pageDraftCreateRequests.get(id);
      if (
        existing.status === "draft"
        && request !== null
        && request !== undefined
        && request.title === title
        && request.content === content
        && request.space === requestedSpace
      ) {
        return { ...existing };
      }
      throw new Error(JSON.stringify({
        code: "page_draft_id_conflict",
        error: "Page draft id already belongs to another Page",
      }));
    }
    if (this.pageDraftCreateRequests.has(id)) {
      throw new Error(JSON.stringify({
        code: "page_draft_id_conflict",
        error: "Page draft id was already used",
      }));
    }
    const space = this.registeredDraftSpace(args);
    const now = "2026-07-10T12:30:00Z";
    const page: KnowledgePage = {
      id,
      title,
      summary: null,
      content,
      entity_id: null,
      domain: space,
      space,
      source_memory_ids: [],
      version: 1,
      status: "draft",
      folder_path: optionalString(args, "folderPath"),
      creation_kind: "authored",
      review_status: "unconfirmed",
      created_at: now,
      last_compiled: now,
      last_modified: now,
    };
    this.pages = [page, ...this.pages];
    this.pageDraftCreateRequests.set(id, { title, content, space });
    return { ...page };
  }

  private normalizedDraftSpace(args: unknown): string | null {
    return optionalString(args, "space")?.trim() || null;
  }

  private registeredDraftSpace(args: unknown): string | null {
    const space = this.normalizedDraftSpace(args);
    if (space && !this.spaces.some((candidate) => candidate.name === space)) {
      throw new Error(`Space ${JSON.stringify(space)} is not registered`);
    }
    return space;
  }

  private draftFor(command: string, args: unknown): { page: KnowledgePage; index: number } {
    const id = requiredString(command, args, "id");
    const index = this.pages.findIndex((page) => page.id === id && page.status === "draft");
    const page = this.pages[index];
    if (index < 0 || !page) {
      throw new Error(JSON.stringify({
        code: "page_draft_not_found",
        error: "Page draft not found",
      }));
    }
    return { page, index };
  }

  private assertDraftVersion(
    command: string,
    args: unknown,
    page: KnowledgePage,
  ): void {
    const expectedVersion = requiredNumber(command, args, "expectedVersion");
    if (page.version !== expectedVersion) {
      throw new Error(JSON.stringify({
        code: "draft_version_conflict",
        error: "Page draft changed since it was loaded",
        current_version: page.version,
      }));
    }
  }

  private updatePageDraft(args: unknown): KnowledgePage {
    const command = "update_page_draft";
    const { page, index } = this.draftFor(command, args);
    const expectedVersion = requiredNumber(command, args, "expectedVersion");
    const title = stringValue(command, args, "title");
    const content = stringValue(command, args, "content");
    if (!title.trim() && !sanitizeIngressContent(content).trim()) {
      throw new Error(JSON.stringify({
        code: "invalid_page_draft",
        error: "A Page draft needs a title or content",
      }));
    }
    const requestedSpace = this.normalizedDraftSpace(args);
    if (
      page.version === expectedVersion + 1
      && page.title === title
      && page.content === content
      && page.space === requestedSpace
      && page.domain === requestedSpace
    ) {
      return { ...page };
    }
    this.assertDraftVersion(command, args, page);
    const space = this.registeredDraftSpace(args);
    const updated: KnowledgePage = {
      ...page,
      title,
      content,
      domain: space,
      space,
      version: page.version + 1,
      last_modified: "2026-07-10T12:31:00Z",
    };
    this.pages[index] = updated;
    return { ...updated };
  }

  private publishPageDraft(args: unknown): KnowledgePage {
    const command = "publish_page_draft";
    const id = requiredString(command, args, "id");
    const expectedVersion = requiredNumber(command, args, "expectedVersion");
    const publishedReplay = this.pages.find((page) => page.id === id);
    if (
      publishedReplay?.status === "active"
      && publishedReplay.version === expectedVersion + 1
    ) {
      if (publishedReplay.projection_status === "pending") {
        const recovered = { ...publishedReplay, storage_path: `${publishedReplay.folder_path ? publishedReplay.folder_path + "/" : ""}${publishedReplay.id}.md`, projection_status: "synced", projection_error: null };
        this.pages = this.pages.map(page => page.id === id ? recovered : page);
        return { ...recovered };
      }
      return { ...publishedReplay };
    }
    if (publishedReplay && publishedReplay.version !== expectedVersion) {
      this.assertDraftVersion(command, args, publishedReplay);
    }
    if (publishedReplay?.status === "active") {
      throw new Error(`Page ${id} is not a draft`);
    }
    const { page, index } = this.draftFor(command, args);
    this.assertDraftVersion(command, args, page);
    const suppliedTitle = page.title.trim();
    const content = sanitizeIngressContent(page.content);
    const generatedTitle = suppliedTitle.length === 0;
    if (generatedTitle && !content.trim()) {
      throw new Error(JSON.stringify({
        code: "invalid_page_draft",
        error: "A Page draft needs a title or content",
      }));
    }
    let title = generatedTitle ? generatedDraftTitle(content) : suppliedTitle;
    const space = page.space?.trim() || null;
    const sameScopeActive = this.pages.filter((candidate) =>
      candidate.id !== page.id
      && candidate.status === "active"
      && (candidate.space?.trim() || null) === space
    );
    if (generatedTitle) {
      const occupiedTitles = new Set(sameScopeActive.map((candidate) => candidate.title.trim().toLowerCase()));
      const base = title;
      let suffix = 2;
      while (occupiedTitles.has(title.toLowerCase())) title = `${base} (${suffix++})`;
    } else {
      const conflict = sameScopeActive.find((candidate) => candidate.title.trim().toLowerCase() === title.toLowerCase());
      if (conflict) {
        throw new Error(JSON.stringify({
          code: "page_title_conflict",
          error: "A Page with this title already exists",
          existing_page_id: conflict.id,
          existing_page_title: conflict.title,
        }));
      }
    }
    const published: KnowledgePage = {
      ...page,
      title,
      content,
      status: "active",
      storage_path: `${page.folder_path ? page.folder_path + "/" : ""}${page.id}.md`,
      projection_status: "synced",
      review_status: "unconfirmed",
      version: page.version + 1,
      last_compiled: "2026-07-10T12:31:00Z",
      last_modified: "2026-07-10T12:31:00Z",
    };
    if (this.pageScenario.projectionPendingOnce && !this.projectionFailureIssued) {
      this.projectionFailureIssued = true;
      published.storage_path = null;
      published.projection_status = "pending";
      published.projection_error = "Fixture: chosen folder is unavailable";
    }
    this.pages[index] = published;
    return { ...published };
  }

  private discardPageDraft(args: unknown): null {
    const command = "discard_page_draft";
    const { page, index } = this.draftFor(command, args);
    this.assertDraftVersion(command, args, page);
    this.pages.splice(index, 1);
    this.pageDraftCreateRequests.set(page.id, null);
    return null;
  }

  private listEntities(args: unknown) {
    const domain = optionalString(args, "domain");
    return this.entityDetails.map((detail) => detail.entity).filter((entity) => !domain || entity.domain === domain || entity.space === domain);
  }

  /**
   * The bulk graph read, derived from the same entity/detail fixtures the
   * per-entity commands serve. A relation is emitted once, from the endpoint
   * that declares it outgoing, and only when both endpoints exist. The
   * navigation fixture links no memory to an entity, so the memory halves are
   * empty rather than invented.
   */
  private getKnowledgeGraph() {
    const entities = this.entityDetails.map((detail) => detail.entity);
    const known = new Set(entities.map((entity) => entity.id));
    const relations: {
      id: string;
      from_entity: string;
      to_entity: string;
      relation_type: string;
      source_agent: string | null;
      created_at: number;
    }[] = [];
    const seen = new Set<string>();
    for (const detail of this.entityDetails) {
      for (const relation of detail.relations) {
        if (!known.has(relation.entity_id) || seen.has(relation.id)) continue;
        seen.add(relation.id);
        const outgoing = relation.direction !== "incoming";
        relations.push({
          id: relation.id,
          from_entity: outgoing ? detail.entity.id : relation.entity_id,
          to_entity: outgoing ? relation.entity_id : detail.entity.id,
          relation_type: relation.relation_type,
          source_agent: relation.source_agent,
          created_at: relation.created_at,
        });
      }
    }
    // Wiki pages and their typed links. The fixture has no wikilink or
    // citation records of its own, so the map half is derived
    // deterministically from the page list: consecutive active pages are
    // wikilinked in a chain, a page carrying `entity_id` gets its `about`
    // link, and the first two pages both cite the first memory (which is what
    // makes the shared-source edge appear while the memory layer is off).
    const activePages = this.pages.filter((page) => page.status === "active");
    const graphPages = activePages.map((page) => ({
      id: page.id,
      title: page.title,
      space: page.space ?? null,
      creation_kind: page.creation_kind ?? "distilled",
      entity_id: page.entity_id ?? null,
      last_modified: page.last_modified,
    }));
    const pageLinks: {
      from: { kind: "page" | "entity" | "memory"; id: string };
      to: { kind: "page" | "entity" | "memory"; id: string };
      link_type: "wikilink" | "about" | "cites";
    }[] = [];
    for (let index = 1; index < activePages.length; index += 1) {
      pageLinks.push({
        from: { kind: "page", id: activePages[index - 1].id },
        to: { kind: "page", id: activePages[index].id },
        link_type: "wikilink",
      });
    }
    for (const page of activePages) {
      if (page.entity_id && known.has(page.entity_id)) {
        pageLinks.push({
          from: { kind: "page", id: page.id },
          to: { kind: "entity", id: page.entity_id },
          link_type: "about",
        });
      }
    }
    const citedMemory = this.memories[0];
    const memories = citedMemory
      ? [
          {
            source_id: citedMemory.source_id,
            title: citedMemory.title,
            memory_type: citedMemory.memory_type,
            space: citedMemory.space,
            confirmed: citedMemory.confirmed,
            last_modified: citedMemory.last_modified,
          },
        ]
      : [];
    if (citedMemory) {
      for (const page of activePages.slice(0, 2)) {
        pageLinks.push({
          from: { kind: "page", id: page.id },
          to: { kind: "memory", id: citedMemory.source_id },
          link_type: "cites",
        });
      }
    }

    return {
      entities,
      relations,
      memories,
      memory_links: [],
      pages: graphPages,
      page_links: pageLinks,
    };
  }

  private getEntityDetail(args: unknown): EntityDetail {
    const entityId = requiredString("get_entity_detail_cmd", args, "entityId");
    const detail = this.entityDetails.find((candidate) => candidate.entity.id === entityId);
    if (!detail) throw new TauriMockArgumentError("get_entity_detail_cmd", "entityId");
    return detail;
  }

  private sourceAnnotationScope(
    command: string,
    source: unknown,
    sourceIdValue: unknown,
    spaceValue: unknown,
  ): { sourceId: string; space: string | null } {
    const sourceKind = requiredString(command, { source }, "source");
    const sourceId = requiredString(command, { sourceId: sourceIdValue }, "sourceId");
    if (sourceKind !== "webpage") throw new Error("unsupported source kind; expected webpage");
    if (sourceId !== sourceId.trim()) {
      throw new Error("source_id must preserve the exact source identity without surrounding whitespace");
    }
    if (spaceValue !== undefined && spaceValue !== null && typeof spaceValue !== "string") {
      throw new TauriMockArgumentError(command, "space");
    }
    const space = typeof spaceValue === "string" ? spaceValue : null;
    if (space !== null) {
      if (!space.trim()) throw new Error("Space must not be blank");
      if (!this.spaces.some((candidate) => candidate.name === space)) {
        throw new Error(`Unknown Space: ${space}`);
      }
    }
    const visible = this.fixture.documents?.some(({ file }) =>
      file.source === "webpage"
      && file.source_id === sourceId
      && (space === null || file.space === space || file.domain === space),
    ) ?? false;
    const ingested = space === null && this.webpages.has(sourceId);
    if (!visible && !ingested) throw new Error("source not found");
    return { sourceId, space };
  }

  private sourceAnnotationPages(pageIds: readonly string[], space: string | null): SourceAnnotation["pages"] {
    return pageIds.flatMap((id) => {
      const page = this.pages.find((candidate) =>
        candidate.id === id
        && candidate.status === "active"
        && (space === null || candidate.space === space || candidate.domain === space),
      );
      return page ? [{ id: page.id, title: page.title }] : [];
    });
  }

  private getSourceAnnotation(args: unknown): SourceAnnotation {
    const { sourceId, space } = this.sourceAnnotationScope(
      "get_source_annotation",
      optionalValue(args, "source"),
      optionalValue(args, "sourceId"),
      optionalValue(args, "space"),
    );
    const key = `${sourceId}\u0000${space ?? ""}`;
    const saved = this.sourceAnnotations.get(key);
    if (!saved) {
      return { source: "webpage", source_id: sourceId, notes: "", page_ids: [], version: 0, updated_at: 0, pages: [] };
    }
    const pages = this.sourceAnnotationPages(saved.page_ids, space);
    const pageIds = pages.map((page) => page.id);
    return { ...saved, page_ids: pageIds, pages };
  }

  private updateSourceAnnotation(args: unknown): SourceAnnotation {
    const command = "update_source_annotation";
    const request = optionalValue(args, "req") as UpdateSourceAnnotationRequest | undefined;
    if (!request || typeof request !== "object") throw new TauriMockArgumentError(command, "req");
    const { sourceId, space } = this.sourceAnnotationScope(
      command,
      request.source,
      request.source_id,
      request.space,
    );
    if (!Number.isSafeInteger(request.expected_version) || request.expected_version < 0) {
      throw new TauriMockArgumentError(command, "req.expected_version");
    }
    if (request.notes !== undefined && typeof request.notes !== "string") {
      throw new TauriMockArgumentError(command, "req.notes");
    }
    if (request.notes !== undefined && new TextEncoder().encode(request.notes).length > 65_536) {
      throw new Error("notes exceed 65536 UTF-8 bytes");
    }
    if (request.page_ids !== undefined && !Array.isArray(request.page_ids)) {
      throw new TauriMockArgumentError(command, "req.page_ids");
    }
    const requestedPageIds = request.page_ids ?? [];
    if (requestedPageIds.length > 100) throw new Error("page_ids exceed 100 entries");
    if (requestedPageIds.some((id) => typeof id !== "string" || !id.trim())
      || new Set(requestedPageIds).size !== requestedPageIds.length) {
      throw new Error("page_ids must be nonblank and unique");
    }
    const key = `${sourceId}\u0000${space ?? ""}`;
    const current = this.sourceAnnotations.get(key)
      ?? { source: "webpage", source_id: sourceId, notes: "", page_ids: [], version: 0, updated_at: 0, pages: [] };
    if (current.version !== request.expected_version) {
      throw new Error(`409 Conflict: source annotation version conflict; current version is ${current.version}`);
    }
    const pageIds = request.page_ids === undefined ? [...current.page_ids] : [...requestedPageIds];
    const pages = this.sourceAnnotationPages(pageIds, space);
    if (request.page_ids !== undefined && pages.length !== pageIds.length) {
      throw new Error("page_ids must reference active Pages visible in this scope");
    }
    const saved: SourceAnnotation = {
      source: "webpage",
      source_id: sourceId,
      notes: request.notes ?? current.notes,
      page_ids: pageIds,
      version: current.version + 1,
      updated_at: Math.floor(Date.now() / 1000),
      pages,
    };
    this.sourceAnnotations.set(key, saved);
    const visiblePages = this.sourceAnnotationPages(saved.page_ids, space);
    return { ...structuredClone(saved), page_ids: visiblePages.map((page) => page.id), pages: visiblePages };
  }

  private addObservation(args: unknown): string {
    const entityId = requiredString("add_observation_cmd", args, "entityId");
    const detail = this.entityDetails.find((candidate) => candidate.entity.id === entityId);
    if (!detail) throw new TauriMockArgumentError("add_observation_cmd", "entityId");
    let id: string;
    do { id = `obs-${++this.observationSequence}`; } while (this.observationIds.has(id));
    this.observationIds.add(id);
    detail.observations.push({ id, entity_id: entityId, content: requiredString("add_observation_cmd", args, "content"), source_agent: optionalString(args, "sourceAgent"), confidence: null, confirmed: false, created_at: 1_783_728_000 });
    return id;
  }

  private updateObservation(args: unknown): null {
    const observationId = requiredString("update_observation_cmd", args, "observationId");
    const content = stringValue("update_observation_cmd", args, "content");
    for (const detail of this.entityDetails) detail.observations = detail.observations.map((item) => item.id === observationId ? { ...item, content } : item);
    return null;
  }

  private deleteObservation(args: unknown): null {
    const observationId = requiredString("delete_observation_cmd", args, "observationId");
    for (const detail of this.entityDetails) detail.observations = detail.observations.filter((item) => item.id !== observationId);
    return null;
  }

  private confirmObservation(args: unknown): null {
    const observationId = requiredString("confirm_observation_cmd", args, "observationId");
    const confirmed = requiredBoolean("confirm_observation_cmd", args, "confirmed");
    for (const detail of this.entityDetails) detail.observations = detail.observations.map((item) => item.id === observationId ? { ...item, confirmed } : item);
    return null;
  }

  private confirmEntity(args: unknown): null {
    const entityId = requiredString("confirm_entity_cmd", args, "entityId");
    const confirmed = requiredBoolean("confirm_entity_cmd", args, "confirmed");
    this.entityDetails = this.entityDetails.map((detail) => detail.entity.id === entityId ? { ...detail, entity: { ...detail.entity, confirmed } } : detail);
    return null;
  }

  private changeEntityType(args: unknown): null {
    const entityId = requiredString("change_entity_type_cmd", args, "entityId");
    const entityType = requiredString("change_entity_type_cmd", args, "entityType");
    this.entityDetails = this.entityDetails.map((detail) => detail.entity.id === entityId
      ? { ...detail, entity: { ...detail.entity, entity_type: entityType } }
      : detail);
    return null;
  }

  private deleteEntity(args: unknown): null {
    const entityId = requiredString("delete_entity_cmd", args, "entityId");
    this.entityDetails = this.entityDetails.filter((detail) => detail.entity.id !== entityId);
    return null;
  }

  /** Shared by `query_entities_cmd` and the `filter` half of a bulk selection:
   * every clause is optional and unset ones pass everything through, matching
   * `entity_filter_conditions` in wenlan-core/src/db.rs. */
  private entityMatchesFilter(entity: Entity, filter: ListEntitiesRequest): boolean {
    if (filter.status && entity.status !== filter.status) return false;
    if (filter.entity_type && entity.entity_type !== filter.entity_type) return false;
    if (filter.space && entity.space !== filter.space && entity.domain !== filter.space) return false;
    if (typeof filter.min_memories === "number" && entity.memory_count < filter.min_memories) return false;
    if (typeof filter.max_memories === "number" && entity.memory_count > filter.max_memories) return false;
    const needle = filter.query?.trim().toLowerCase();
    if (needle && !entity.name.toLowerCase().includes(needle)) return false;
    return true;
  }

  /** The Entities view's paged reader (#708): all three lifecycle states,
   * filtered and paged, unlike the legacy `list_entities_cmd`. */
  private queryEntities(args: unknown): ListEntitiesResponse {
    const filter = (optionalValue(args, "filter") as ListEntitiesRequest | undefined) ?? {};
    const matches = this.entityDetails
      .map((detail) => detail.entity)
      .filter((entity) => this.entityMatchesFilter(entity, filter));
    const offset = filter.offset ?? 0;
    const limit = filter.limit ?? 100;
    return { entities: matches.slice(offset, offset + limit), total: matches.length };
  }

  /** Resolves an `EntitySelection` (`ids` xor `filter`) against entities that
   * are actually eligible for the bulk action, mirroring the daemon's
   * skip-don't-fail selection in `select_bulk_entity_ids`. */
  private selectEligibleEntities(
    selection: { ids?: string[]; filter?: ListEntitiesRequest },
    eligible: (entity: Entity) => boolean,
  ): Entity[] {
    const pool = this.entityDetails.map((detail) => detail.entity).filter(eligible);
    if (selection.ids) {
      const ids = new Set(selection.ids);
      return pool.filter((entity) => ids.has(entity.id));
    }
    if (selection.filter) {
      const filter = selection.filter;
      return pool.filter((entity) => this.entityMatchesFilter(entity, filter));
    }
    return [];
  }

  /** Archives every eligible (non-archived) selected entity (#708);
   * `confirmed`/`established_by` are untouched, exactly as `archive_entity_in_transaction`
   * only flips the page's `status` column. `dry_run` previews without mutating. */
  private archiveEntities(args: unknown): EntityBulkResponse {
    const req = optionalValue(args, "req") as ArchiveEntitiesRequest;
    const selected = this.selectEligibleEntities(req, (entity) => entity.status !== "archived");
    if (!req.dry_run) {
      const ids = new Set(selected.map((entity) => entity.id));
      this.entityDetails = this.entityDetails.map((detail) =>
        ids.has(detail.entity.id)
          ? { ...detail, entity: { ...detail.entity, status: "archived" } }
          : detail,
      );
    }
    return { count: selected.length, entity_ids: selected.map((entity) => entity.id), dry_run: req.dry_run };
  }

  /** Exact inverse of {@link archiveEntities}: only archived entities are
   * eligible, and each reverts to established or detected by its own
   * `confirmed` flag, not unconditionally to detected. */
  private restoreEntities(args: unknown): EntityBulkResponse {
    const req = optionalValue(args, "req") as RestoreEntitiesRequest;
    const selected = this.selectEligibleEntities(req, (entity) => entity.status === "archived");
    if (!req.dry_run) {
      const ids = new Set(selected.map((entity) => entity.id));
      this.entityDetails = this.entityDetails.map((detail) =>
        ids.has(detail.entity.id)
          ? { ...detail, entity: { ...detail.entity, status: detail.entity.confirmed ? "established" : "detected" } }
          : detail,
      );
    }
    return { count: selected.length, entity_ids: selected.map((entity) => entity.id), dry_run: req.dry_run };
  }

  private listMemories(args: unknown): readonly MemoryItem[] {
    const domain = optionalString(args, "domain");
    const limit = optionalValue(args, "limit");
    const filtered = this.memories.filter((memory) => !domain || memory.domain === domain || memory.space === domain);
    return typeof limit === "number" ? filtered.slice(0, limit) : filtered;
  }

  private listIndexedFiles(): readonly Record<string, unknown>[] {
    const memories = this.memories.map((memory) => ({
      source: "review-fixture.md",
      source_id: memory.source_id,
      title: memory.title,
      summary: memory.summary,
      chunk_count: memory.chunk_count,
      last_modified: memory.last_modified,
      processing: false,
      memory_type: memory.memory_type,
      domain: memory.domain,
      space: memory.space,
      source_agent: memory.source_agent,
      confidence: memory.confidence,
      confirmed: memory.confirmed,
      pinned: memory.pinned,
    }));
    return [...memories, ...(this.fixture.documents ?? []).map(document => ({ ...document.file })), ...Array.from(this.webpages.values(), (webpage) => ({
      source: "webpage", source_id: webpage.url, title: webpage.title,
      url: webpage.url, chunk_count: 1, last_modified: 1_783_728_000,
      summary: null, processing: false,
    }))];
  }

  private ingestWebpage(args: unknown): { chunks_created: number; document_id: string } {
    const request = optionalValue(args, "req");
    const url = requiredString("ingest_webpage", request, "url");
    const title = requiredString("ingest_webpage", request, "title");
    const content = requiredString("ingest_webpage", request, "content");
    const createOnly = optionalValue(request, "create_only");
    if (createOnly !== undefined && typeof createOnly !== "boolean") throw new TauriMockArgumentError("ingest_webpage", "create_only");
    if (createOnly && this.webpages.has(url)) throw new Error("WEBPAGE_ALREADY_EXISTS");
    this.webpages.set(url, { url, title, content });
    return { chunks_created: 1, document_id: url };
  }

  private updateMemory(args: unknown): null {
    const sourceId = requiredString("update_memory_cmd", args, "sourceId");
    const content = optionalValue(args, "content");
    const domain = optionalValue(args, "domain");
    const confirmed = optionalValue(args, "confirmed");
    const memoryType = optionalValue(args, "memoryType");
    if (content !== null && content !== undefined && typeof content !== "string") {
      throw new TauriMockArgumentError("update_memory_cmd", "content");
    }
    if (domain !== null && domain !== undefined && typeof domain !== "string") {
      throw new TauriMockArgumentError("update_memory_cmd", "domain");
    }
    if (confirmed !== null && confirmed !== undefined && typeof confirmed !== "boolean") {
      throw new TauriMockArgumentError("update_memory_cmd", "confirmed");
    }
    if (memoryType !== null && memoryType !== undefined && typeof memoryType !== "string") {
      throw new TauriMockArgumentError("update_memory_cmd", "memoryType");
    }
    if (!this.memories.some((memory) => memory.source_id === sourceId)) {
      throw new TauriMockArgumentError("update_memory_cmd", "sourceId");
    }
    this.memories = this.memories.map((memory) =>
      memory.source_id === sourceId
        ? {
            ...memory,
            ...(typeof content === "string" ? { content } : {}),
            ...(typeof domain === "string" ? { domain, space: domain } : {}),
            ...(typeof confirmed === "boolean" ? { confirmed } : {}),
            ...(typeof memoryType === "string" ? { memory_type: memoryType } : {}),
          }
        : memory
    );
    return null;
  }

  private reclassifyMemory(args: unknown): string {
    const sourceId = requiredString("reclassify_memory_cmd", args, "sourceId");
    const memoryType = requiredString("reclassify_memory_cmd", args, "memoryType");
    if (!this.memories.some((memory) => memory.source_id === sourceId)) {
      throw new TauriMockArgumentError("reclassify_memory_cmd", "sourceId");
    }
    this.memories = this.memories.map((memory) =>
      memory.source_id === sourceId ? { ...memory, memory_type: memoryType } : memory
    );
    return sourceId;
  }

  private setStability(args: unknown): null {
    const sourceId = requiredString("set_stability_cmd", args, "sourceId");
    const stability = requiredString("set_stability_cmd", args, "stability");
    if (!["new", "learned", "confirmed"].includes(stability)) {
      throw new TauriMockArgumentError("set_stability_cmd", "stability");
    }
    if (!this.memories.some((memory) => memory.source_id === sourceId)) {
      throw new TauriMockArgumentError("set_stability_cmd", "sourceId");
    }
    this.memories = this.memories.map((memory) =>
      memory.source_id === sourceId
        ? {
            ...memory,
            stability: stability as "new" | "learned" | "confirmed",
            confirmed: stability === "confirmed",
          }
        : memory
    );
    return null;
  }

  private setMemoryPinned(args: unknown, pinned: boolean): null {
    const command = pinned ? "pin_memory" : "unpin_memory";
    const sourceId = requiredString(command, args, "sourceId");
    if (!this.memories.some((memory) => memory.source_id === sourceId)) {
      throw new TauriMockArgumentError(command, "sourceId");
    }
    this.memories = this.memories.map((memory) =>
      memory.source_id === sourceId ? { ...memory, pinned } : memory
    );
    return null;
  }

  private deleteFileChunks(args: unknown): null {
    const source = requiredString("delete_file_chunks", args, "source");
    const sourceId = requiredString("delete_file_chunks", args, "sourceId");
    if (source !== "memory") {
      throw new TauriMockArgumentError("delete_file_chunks", "source");
    }
    this.memories = this.memories.filter((memory) => memory.source_id !== sourceId);
    return null;
  }

  private confirmMemory(args: unknown): null {
    const sourceId = requiredString("confirm_memory", args, "sourceId");
    const confirmed = requiredBoolean("confirm_memory", args, "confirmed");
    this.memories = this.memories.map((memory) =>
      memory.source_id === sourceId ? { ...memory, confirmed } : memory
    );
    return null;
  }

  private deleteMemory(args: unknown): null {
    const sourceId = requiredString("delete_memory", args, "sourceId");
    this.memories = this.memories.filter((memory) => memory.source_id !== sourceId);
    return null;
  }

  private search(args: unknown) {
    const query = requiredString("search", args, "query").toLowerCase();
    const limit = optionalValue(args, "limit");
    const results = this.memories.filter((memory) => `${memory.title} ${memory.content}`.toLowerCase().includes(query) || query.includes("ada"));
    return results.slice(0, typeof limit === "number" ? limit : 10).map((memory, index) => ({ id: `chunk-${index}`, content: memory.content, source: "fixture.md", source_id: memory.source_id, title: memory.title, url: null, chunk_index: 0, last_modified: memory.last_modified, score: 0.94, memory_type: memory.memory_type, entity_id: query.includes("ada") ? "entity-ada" : null, is_archived: false }));
  }

}
