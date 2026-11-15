// SPDX-License-Identifier: Apache-2.0
// Local fixture only: never import into the production entrypoint.
import { oauthStore } from '../../src/oauth-store.ts';
export async function inspectOAuth(request: Request, state: DurableObjectState): Promise<Response | null> {
  if (new URL(request.url).pathname !== '/__oauth_storage') return null;
  const { operation, args } = await request.json() as { operation: string; args: any[] };
  const store = oauthStore(state.storage.sql);
  if (!['get', 'put', 'delete', 'list'].includes(operation)) return new Response(null, { status: 400 });
  const result = await (store[operation as 'get'] as Function)(...args);
  return Response.json(result ?? null);
}

export function readOnlyLegacy(kv: KVNamespace): KVNamespace {
  return new Proxy(kv, { get(target, key) {
    if (['list', 'put', 'delete'].includes(String(key))) return () => { throw new Error(`Forbidden legacy KV ${String(key)}`); };
    const value = Reflect.get(target, key);
    return typeof value === 'function' ? value.bind(target) : value;
  } });
}
