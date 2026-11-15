// SPDX-License-Identifier: Apache-2.0
// Local fixture only. Production contains no inspection route.
import { RelayAuthority as Authority } from '../../src/worker.ts';
import { inspectOAuth, readOnlyLegacy } from './oauth-storage.ts';
export { default } from '../../src/worker.ts';
export class RelayAuthority extends Authority {
  constructor(private fixtureState: DurableObjectState, env: ConstructorParameters<typeof Authority>[1]) {
    super(fixtureState, { ...env, OAUTH_KV: readOnlyLegacy(env.OAUTH_KV) });
  }
  async fetch(request: Request) { return await inspectOAuth(request, this.fixtureState) ?? super.fetch(request); }
}
