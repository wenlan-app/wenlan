// SPDX-License-Identifier: Apache-2.0
// Synthetic runtime fixture only. Never deploy these state/fault controls.
import { RelayAuthority } from '../../src/worker.ts';
import { boundedStore } from '../../src/bounded-store.ts';
import { inspectOAuth, readOnlyLegacy } from './oauth-storage.ts';
export { default } from '../../src/worker.ts';

export class MaintenanceFixture extends RelayAuthority {
  private fixtureState: DurableObjectState;
  constructor(state: DurableObjectState, env: ConstructorParameters<typeof RelayAuthority>[1]) {
    super(state, { ...env, OAUTH_KV: readOnlyLegacy(env.OAUTH_KV) });
    this.fixtureState = state;
  }
  async fetch(request: Request): Promise<Response> {
    const inspected = await inspectOAuth(request, this.fixtureState);
    if (inspected) return inspected;
    if (new URL(request.url).pathname !== '/__fixture') return super.fetch(request);
    const { operation, records } = await request.json() as { operation: string; records?: Record<string, unknown> };
    if (operation === 'seed') await boundedStore(this.fixtureState.storage).transaction(async tx => {
      for (const [key, value] of Object.entries(records!)) await tx.put(key, value);
    });
    else if (operation === 'oauth-expired' || operation === 'oauth-full') {
      this.fixtureState.storage.sql.exec(`WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x < ?) INSERT INTO oauth_kv(key, value, expires) SELECT printf('fixture:%05d', x), '{}', ? FROM n`,
        operation === 'oauth-full' ? 50000 : 300, operation === 'oauth-full' ? null : Math.floor(Date.now() / 1000) - 1);
      this.fixtureState.storage.sql.exec('UPDATE oauth_meta SET rows = (SELECT count(*) FROM oauth_kv) + (SELECT count(*) FROM oauth_migrated) WHERE id = 1');
    }
    else if (operation === 'alarm') await this.alarm();
    else if (operation === 'schedule') await this.fixtureState.storage.setAlarm(Date.now() + 25);
    else if (operation === 'clear') {
      const keys = [...(await this.fixtureState.storage.list({ limit: 128 })).keys()];
      if (keys.length) await this.fixtureState.storage.delete(keys);
      this.fixtureState.storage.sql.exec('DELETE FROM rate_window');
      this.fixtureState.storage.sql.exec('DELETE FROM oauth_kv');
      this.fixtureState.storage.sql.exec('DELETE FROM oauth_migrated');
      this.fixtureState.storage.sql.exec('UPDATE oauth_meta SET rows = 0 WHERE id = 1');
    } else if (operation !== 'inspect') return new Response(null, { status: 400 });
    return Response.json({ records: Object.fromEntries(await this.fixtureState.storage.list({ limit: 128 })),
      oauthRows: this.fixtureState.storage.sql.exec('SELECT count(*) AS n FROM oauth_kv').one().n,
      alarm: await this.fixtureState.storage.getAlarm() });
  }
}
