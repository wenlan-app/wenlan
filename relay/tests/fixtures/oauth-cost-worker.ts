// SPDX-License-Identifier: Apache-2.0
// Local SQL billing probe only; never imported by the production worker.
import { DurableObject } from 'cloudflare:workers';
import { oauthStore } from '../../src/oauth-store.ts';

export class OAuthCostFixture extends DurableObject {
  async fetch() {
    const cursors: SqlStorageCursor[] = [];
    const raw = this.ctx.storage.sql;
    const sql = { exec(query: string, ...bindings: any[]) {
      const cursor = raw.exec(query, ...bindings);
      cursors.push(cursor);
      return cursor;
    } } as SqlStorage;
    const store = oauthStore(sql, { get: async () => null } as unknown as KVNamespace, () => 1);
    raw.exec(`WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x < 5000)
      INSERT INTO oauth_kv(key, value) SELECT printf('other:%05d', x), 'x' FROM n`);
    raw.exec('UPDATE oauth_meta SET rows = 5000 WHERE id = 1');
    const measure = async (operation: () => Promise<unknown>) => {
      cursors.length = 0;
      await operation();
      return cursors.reduce((n, cursor) => n + cursor.rowsRead, 0);
    };
    const put = await measure(() => store.put('token:s:g:new', 'x'));
    const overwrite = await measure(() => store.put('token:s:g:new', 'y'));
    const list = await measure(() => store.list({ prefix: 'token:s:g:' }));
    const deletePrefix = await measure(() => store.deletePrefix('token:s:g:'));
    const marked = await measure(() => store.get('token:s:g:unread'));
    const miss = await measure(() => store.get('unknown'));
    return Response.json({ put, overwrite, list, deletePrefix, marked, miss,
      count: raw.exec('SELECT rows FROM oauth_meta WHERE id = 1').one().rows,
      actual: raw.exec('SELECT (SELECT count(*) FROM oauth_kv) + (SELECT count(*) FROM oauth_migrated) AS n').one().n });
  }
}
export default { fetch: () => new Response(null, { status: 404 }) };
