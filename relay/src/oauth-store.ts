// SPDX-License-Identifier: Apache-2.0
/** Legacy KV is read-only during this fixed migration window. */
export const OAUTH_MIGRATION_CUTOFF = Date.parse('2026-12-16T00:00:00Z') / 1000;
export const OAUTH_ROW_LIMIT = 50_000;
const THIRTY_DAYS = 30 * 86400;
const unsupported = () => { throw new Error('Unsupported OAuth storage operation'); };

type PutOptions = { expiration?: number; expirationTtl?: number; metadata?: unknown };
type Row = { key: string; value: string; metadata: string | null; expires: number | null; legacy: number };

/** Only the operations used by workers-oauth-provider@1.2.1 are supported.
 * All SQL mutations are synchronous; a legacy read rechecks SQL after awaiting.
 * Markers protect deleted legacy keys until cutoff; misses stay in memory.
 */
export class OAuthStore {
  private sql: SqlStorage;
  private legacy?: KVNamespace;
  private clock: () => number;
  private cap: number;
  private pending = new Map<string, Promise<void>>();
  private cancelled = new Set<string>();
  private misses = new Map<string, number>();
  constructor(sql: SqlStorage, legacy?: KVNamespace,
    clock = () => Date.now() / 1000, cap = OAUTH_ROW_LIMIT) {
    this.sql = sql; this.legacy = legacy; this.clock = clock; this.cap = cap;
    sql.exec('CREATE TABLE IF NOT EXISTS oauth_kv (key TEXT PRIMARY KEY, value TEXT NOT NULL, metadata TEXT, expires INTEGER, legacy INTEGER NOT NULL DEFAULT 0)');
    sql.exec('CREATE INDEX IF NOT EXISTS oauth_expiry ON oauth_kv(expires)');
    sql.exec('CREATE TABLE IF NOT EXISTS oauth_migrated (key TEXT PRIMARY KEY, prefix INTEGER NOT NULL)');
    sql.exec('CREATE TABLE IF NOT EXISTS oauth_meta (id INTEGER PRIMARY KEY CHECK (id = 1), rows INTEGER NOT NULL)');
    if (!sql.exec('SELECT rows FROM oauth_meta WHERE id = 1').toArray().length) {
      sql.exec('INSERT INTO oauth_meta(id, rows) SELECT 1, (SELECT count(*) FROM oauth_kv) + (SELECT count(*) FROM oauth_migrated)');
    }
  }

  private row(key: string) { return this.sql.exec<Row>('SELECT * FROM oauth_kv WHERE key = ?', key).toArray()[0]; }
  private migrating() { return this.clock() < OAUTH_MIGRATION_CUTOFF; }
  private marked(key: string) {
    if (this.sql.exec('SELECT key FROM oauth_migrated WHERE key = ?', key).toArray().length) return true;
    const token = /^token:([^:]+):([^:]+):[^:]+$/.exec(key);
    return !!token && this.sql.exec('SELECT key FROM oauth_migrated WHERE key = ? AND prefix = 1',
      `token:${token[1]}:${token[2]}:`).toArray().length > 0;
  }
  private mutate(query: string, ...bindings: (string | number)[]) {
    this.sql.exec(query, ...bindings);
    // changes() counts table rows, excluding index maintenance writes.
    return this.sql.exec<{ n: number }>('SELECT changes() AS n').one().n;
  }
  private adjust(delta: number) {
    if (delta) this.sql.exec('UPDATE oauth_meta SET rows = rows + ? WHERE id = 1', delta);
  }
  private room(additional: () => number) {
    const count = () => this.sql.exec<{ rows: number }>('SELECT rows FROM oauth_meta WHERE id = 1').one().rows;
    if (count() + additional() <= this.cap) return;
    // Admission sweep is exhaustive only at capacity. Alarm cleanup is bounded.
    this.expire();
    if (!this.migrating()) this.adjust(-this.mutate('DELETE FROM oauth_migrated'));
    if (count() + additional() > this.cap) throw new Error('OAuth storage capacity reached');
  }
  private mark(key: string, prefix = false) {
    const exists = () => this.sql.exec('SELECT key FROM oauth_migrated WHERE key = ?', key).toArray().length > 0;
    this.room(() => exists() ? 0 : 1);
    const present = exists();
    this.sql.exec('INSERT INTO oauth_migrated(key, prefix) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET prefix = max(prefix, excluded.prefix)', key, prefix ? 1 : 0);
    if (!present) this.adjust(1);
  }
  private expire(limit = -1) {
    const bindings = [this.clock(), limit];
    // Replacing legacy rows with markers preserves capacity, even at the cap.
    if (this.migrating()) this.adjust(this.mutate(`INSERT OR IGNORE INTO oauth_migrated(key, prefix)
      SELECT key, 0 FROM oauth_kv WHERE key IN
      (SELECT key FROM oauth_kv WHERE expires <= ? LIMIT ?) AND legacy = 1`, ...bindings));
    this.adjust(-this.mutate('DELETE FROM oauth_kv WHERE key IN (SELECT key FROM oauth_kv WHERE expires <= ? LIMIT ?)', ...bindings));
  }
  private missed(key: string) {
    const until = this.misses.get(key);
    if (until === undefined) return false;
    if (until > this.clock()) return true;
    this.misses.delete(key);
    return false;
  }
  private cancel(key: string) {
    if (this.pending.has(key)) this.cancelled.add(key);
    this.misses.delete(key);
  }

  async get(key: string, options?: 'json' | { type?: 'json' | 'text' }): Promise<any> {
    const type = typeof options === 'string' ? options : options?.type;
    if (type !== undefined && type !== 'text' && type !== 'json') return unsupported();
    if (!this.row(key) && this.migrating() && this.legacy && !this.missed(key) && !this.marked(key)) {
      let pending = this.pending.get(key);
      if (!pending) {
        pending = this.migrate(key);
        this.pending.set(key, pending);
      }
      try { await pending; } finally { if (this.pending.get(key) === pending) { this.pending.delete(key); this.cancelled.delete(key); } }
    }
    const row = this.row(key);
    if (!row || (row.expires !== null && row.expires <= this.clock())) return null;
    return type === 'json' ? JSON.parse(row.value) : row.value;
  }

  private async migrate(key: string): Promise<void> {
    const value = await this.legacy!.get(key);
    // A concurrent put/delete wins over the legacy read.
    if (this.cancelled.has(key) || this.row(key) || this.marked(key) || !this.migrating()) return;
    if (value === null) {
      if (this.misses.size >= 1000) this.misses.delete(this.misses.keys().next().value!);
      this.misses.set(key, this.clock() + 600);
      return;
    }
    let expires = this.clock() + THIRTY_DAYS;
    try {
      const record = JSON.parse(value);
      // Client registrations use registrationExpiresAt; grants/tokens use expiresAt.
      const ownExpiry = record?.expiresAt ?? record?.registrationExpiresAt;
      if (typeof ownExpiry === 'number' && Number.isFinite(ownExpiry)) expires = ownExpiry;
    } catch { /* JTI markers and other text receive the migration cap. */ }
    this.write(key, value, null, Math.max(0, Math.floor(expires)), true);
  }

  async put(key: string, value: string, options: PutOptions = {}): Promise<void> {
    if (typeof value !== 'string') return unsupported();
    const expires = options.expiration ?? (options.expirationTtl === undefined ? null : Math.floor(this.clock() + options.expirationTtl));
    const metadata = options.metadata === undefined ? null : JSON.stringify(options.metadata);
    this.write(key, value, metadata, expires);
  }

  private write(key: string, value: string, metadata: string | null, expires: number | null, legacy = false) {
    if (expires !== null && (!Number.isSafeInteger(expires) || expires < 0)) throw new Error('Invalid OAuth expiration');
    this.room(() => this.row(key) ? 0 : 1);
    const previous = this.row(key);
    this.cancel(key);
    this.sql.exec('INSERT INTO oauth_kv(key, value, metadata, expires, legacy) VALUES (?, ?, ?, ?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value, metadata = excluded.metadata, expires = excluded.expires', key, value, metadata, expires, legacy || previous?.legacy ? 1 : 0);
    if (!previous) this.adjust(1);
  }

  async delete(key: string): Promise<void> {
    const previous = this.row(key);
    this.cancel(key);
    this.adjust(-this.mutate('DELETE FROM oauth_kv WHERE key = ?', key));
    if (this.migrating() && (!previous || previous.legacy)) this.mark(key);
  }

  /** Also blocks migration of legacy tokens that have never been read. */
  async deletePrefix(prefix: string): Promise<void> {
    for (const key of this.pending.keys()) if (key >= prefix && key < prefix + '\uffff') this.cancel(key);
    if (this.migrating() && !/^token:[^:]+:[^:]+:$/.test(prefix)) {
      this.adjust(this.mutate('INSERT OR IGNORE INTO oauth_migrated(key, prefix) SELECT key, 0 FROM oauth_kv WHERE key >= ? AND key < ? AND legacy = 1', prefix, prefix + '\uffff'));
    }
    this.adjust(-this.mutate('DELETE FROM oauth_kv WHERE key >= ? AND key < ?', prefix, prefix + '\uffff'));
    // The prefix itself is an explicit deletion, including unread legacy tokens.
    if (this.migrating()) this.mark(prefix, true);
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
    const rows = this.sql.exec<Row>('SELECT * FROM oauth_kv WHERE key >= ? AND key < ? AND (? IS NULL OR key > ?) AND (expires IS NULL OR expires > ?) ORDER BY key LIMIT ?', prefix, prefix + '\uffff', after, after, this.clock(), limit + 1).toArray();
    const complete = rows.length <= limit;
    const page = rows.slice(0, limit);
    return { keys: page.map(row => ({ name: row.key, ...(row.expires === null ? {} : { expiration: row.expires }),
      ...(row.metadata === null ? {} : { metadata: JSON.parse(row.metadata) }) })), list_complete: complete,
      cursor: complete ? '' : btoa(encodeURIComponent(JSON.stringify([prefix, page.at(-1)!.key]))) };
  }

  /** Return whether retention work remains, including future expirations. */
  sweep(limit = 256): boolean {
    this.expire(limit);
    if (!this.migrating()) this.adjust(-this.mutate('DELETE FROM oauth_migrated WHERE key IN (SELECT key FROM oauth_migrated LIMIT ?)', limit));
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
