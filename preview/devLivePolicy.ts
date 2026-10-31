// SPDX-License-Identifier: AGPL-3.0-only
// The live dev window can browse the existing daemon, never edit its data.
const READ_POSTS = new Set([
  "/api/search",
  "/api/pages/search",
  "/api/memory/list",
  "/api/memory/entities/list",
  "/api/memory/entities/search",
  "/api/memory/entities/query",
  // A Brief read: the POST carries the Space name; it never writes a Brief.
  "/api/brief",
]);

export function allowsDevLiveRequest(method: string, path: string): boolean {
  if (!path.startsWith("/api/")) return false;
  return method === "GET" || method === "HEAD" || (method === "POST" && READ_POSTS.has(path));
}

export function allowsDevLiveCommand(command: string): boolean {
  return /^(get_|list_|search|query_entities_cmd$|count_knowledge_files$|should_show_wizard$|is_|daemon_version$|page_review_supported$)/.test(command);
}
