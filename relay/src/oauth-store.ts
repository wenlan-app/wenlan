// SPDX-License-Identifier: Apache-2.0
/** Legacy KV is read-only during this fixed migration window. */
export const OAUTH_MIGRATION_CUTOFF = Date.parse('2026-12-16T00:00:00Z') / 1000;
export const OAUTH_ROW_LIMIT = 50_000;
const THIRTY_DAYS = 30 * 86400;
const unsupported = () => { throw new Error('Unsupported OAuth storage operation'); };

type PutOptions = { expiration?: number; expirationTtl?: number; metadata?: unknown };
type Row = { key: string; value: string; metadata: string | null; expires: number | null };

/** Only the operations used by workers-oauth-provider@1.2.1 are supported.
 * All SQL mutations are synchronous; a legacy read rechecks SQL after awaiting.
 * Markers remember touched keys until cutoff, including misses and deletes.
 */
export class OAuthStore {
  private sql: SqlStorage;
  private legacy?: KVNamespace;
  private clock: () => number;
  private cap: number;
  private pending = new Map<string, Promise<void>>();
  constructor(sql: SqlStorage, legacy?: KVNamespace,
    clock = () => Date.now() / 1000, cap = OAUTH_ROW_LIMIT) {
    this.sql = sql; this.legacy = legacy; this.clock = clock; this.cap = cap;
    sql.exec('CREATE TABLE IF NOT EXISTS oauth_kv (key TEXT PRIMARY KEY, value TEXT NOT NULL, metadata TEXT, expires INTEGER)');
    sql.exec('CREATE INDEX IF NOT EXISTS oauth_expiry ON oauth_kv(expires)');
    sql.exec('CREATE TABLE IF NOT EXISTS oauth_migrated (key TEXT PRIMARY KEY, prefix INTEGER NOT NULL)');
  }

  private row(key: string) { return this.sql.exec<Row>('SELECT * FROM oauth_kv WHERE key = ?', key).toArray()[0]; }
  private migrating() { return this.clock() < OAUTH_MIGRATION_CUTOFF; }
  private marked(key: string) {
    return this.sql.exec('SELECT key FROM oauth_migrated WHERE (prefix = 0 AND key = ?) OR (prefix = 1 AND substr(?, 1, length(key)) = key) LIMIT 1', key, key).toArray().length > 0;
  }
  private room(key: string, marker: boolean, value: boolean) {
    const additional = (value && !this.row(key) ? 1 : 0) + (marker && !this.sql.exec('SELECT key FROM oauth_migrated WHERE key = ?', key).toArray().length ? 1 : 0);
    const count = () => this.sql.exec<{ n: number }>('SELECT (SELECT count(*) FROM oauth_kv) + (SELECT count(*) FROM oauth_migrated) AS n').one().n;
    if (count() + additional <= this.cap) return;
    // Admission sweep is exhaustive only at capacity. Alarm cleanup is bounded.
    this.sql.exec('DELETE FROM oauth_kv WHERE expires <= ?', this.clock());
    if (!this.migrating()) this.sql.exec('DELETE FROM oauth_migrated');
    const needed = (value && !this.row(key) ? 1 : 0) + (marker && !this.sql.exec('SELECT key FROM oauth_migrated WHERE key = ?', key).toArray().length ? 1 : 0);
    if (count() + needed > this.cap) throw new Error('OAuth storage capacity reached');
  }
  private mark(key: string, prefix = false) {
    this.sql.exec('INSERT INTO oauth_migrated(key, prefix) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET prefix = max(prefix, excluded.prefix)', key, prefix ? 1 : 0);
  }

  async get(key: string, options?: 'json' | { type?: 'json' | 'text' }): Promise<any> {
    const type = typeof options === 'string' ? options : options?.type;
    if (type !== undefined && type !== 'text' && type !== 'json') return unsupported();
    if (!this.row(key) && this.migrating() && this.legacy && !this.marked(key)) {
      let pending = this.pending.get(key);
      if (!pending) {
        pending = this.migrate(key);
        this.pending.set(key, pending);
      }
      try { await pending; } finally { if (this.pending.get(key) === pending) this.pending.delete(key); }
    }
    const row = this.row(key);
    if (!row || (row.expires !== null && row.expires <= this.clock())) return null;
    return type === 'json' ? JSON.parse(row.value) : row.value;
  }

  private async migrate(key: string): Promise<void> {
    const value = await this.legacy!.get(key);
    // A concurrent put/delete wins over the legacy read.
    if (this.row(key) || this.marked(key) || !this.migrating()) return;
    if (value === null) { this.room(key, true, false); this.mark(key); return; }
    let expires = this.clock() + THIRTY_DAYS;
    try {
      const record = JSON.parse(value);
      // Client registrations use registrationExpiresAt; grants/tokens use expiresAt.
      const ownExpiry = record?.expiresAt ?? record?.registrationExpiresAt;
      if (typeof ownExpiry === 'number' && Number.isFinite(ownExpiry)) expires = ownExpiry;
    } catch { /* JTI markers and other text receive the migration cap. */ }
    await this.put(key, value, { expiration: Math.max(0, Math.floor(expires)) });
  }

  async put(key: string, value: string, options: PutOptions = {}): Promise<void> {
    if (typeof value !== 'string') return unsupported();
    const expires = options.expiration ?? (options.expirationTtl === undefined ? null : Math.floor(this.clock() + options.expirationTtl));
    if (expires !== null && (!Number.isSafeInteger(expires) || expires < 0)) throw new Error('Invalid OAuth expiration');
    const metadata = options.metadata === undefined ? null : JSON.stringify(options.metadata);
    const migrating = this.migrating();
    this.room(key, migrating, true);
    if (migrating) this.mark(key);
    this.sql.exec('INSERT INTO oauth_kv(key, value, metadata, expires) VALUES (?, ?, ?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value, metadata = excluded.metadata, expires = excluded.expires', key, value, metadata, expires);
  }

  async delete(key: string): Promise<void> {
    if (this.migrating()) { this.room(key, true, false); this.mark(key); }
    this.sql.exec('DELETE FROM oauth_kv WHERE key = ?', key);
  }

  /** Also blocks migration of legacy tokens that have never been read. */
  async deletePrefix(prefix: string): Promise<void> {
    if (this.migrating()) { this.room(prefix, true, false); this.mark(prefix, true); }
    this.sql.exec('DELETE FROM oauth_kv WHERE substr(key, 1, length(?)) = ?', prefix, prefix);
  }

  async list(options: { prefix?: string; limit?: number; cursor?: string } = {}) {
    const { prefix = '', limit = 1000, cursor = '' } = options;
    if (!Number.isInteger(limit) || limit < 1 || limit > 1000) throw new Error('Invalid OAuth list limit');
    let after: string | null = null;
    if (cursor) {
      try {
        const decoded = JSON.parse(decodeURIComponent(atob(cursor)));
        if (!Array.isArray(decoded) || decoded.length !== 2 || decoded[0] !== prefix || typeof decoded[1] !== 'string') throw new Error();
        after = decoded[1];
      } catch { throw new Error('Invalid OAuth list cursor'); }
    }
    const rows = this.sql.exec<Row>('SELECT * FROM oauth_kv WHERE substr(key, 1, length(?)) = ? AND (? IS NULL OR key > ?) AND (expires IS NULL OR expires > ?) ORDER BY key LIMIT ?', prefix, prefix, after, after, this.clock(), limit + 1).toArray();
    const complete = rows.length <= limit;
    const page = rows.slice(0, limit);
    return { keys: page.map(row => ({ name: row.key, ...(row.expires === null ? {} : { expiration: row.expires }),
      ...(row.metadata === null ? {} : { metadata: JSON.parse(row.metadata) }) })), list_complete: complete,
      cursor: complete ? '' : btoa(encodeURIComponent(JSON.stringify([prefix, page.at(-1)!.key]))) };
  }

  /** Return whether retention work remains, including future expirations. */
  sweep(limit = 256): boolean {
    this.sql.exec('DELETE FROM oauth_kv WHERE key IN (SELECT key FROM oauth_kv WHERE expires <= ? LIMIT ?)', this.clock(), limit);
    if (!this.migrating()) this.sql.exec('DELETE FROM oauth_migrated WHERE key IN (SELECT key FROM oauth_migrated LIMIT ?)', limit);
    return this.sql.exec('SELECT key FROM oauth_kv WHERE expires IS NOT NULL LIMIT 1').toArray().length > 0
      || this.sql.exec('SELECT key FROM oauth_migrated LIMIT 1').toArray().length > 0;
  }

  getWithMetadata = unsupported;
  getWithMetadataAndCacheStatus = unsupported;
  getWithCacheStatus = unsupported;
}

/** Proxy rejects unknown KV methods rather than silently accepting them. */
export function oauthStore(sql: SqlStorage, legacy?: KVNamespace, clock?: () => number, cap?: number): OAuthStore & KVNamespace {
  const store = new OAuthStore(sql, legacy, clock, cap);
  return new Proxy(store, { get(target, key) {
    if (!(key in target)) return unsupported;
    const value = Reflect.get(target, key);
    return typeof value === 'function' ? value.bind(target) : value;
  } }) as OAuthStore & KVNamespace;
}
