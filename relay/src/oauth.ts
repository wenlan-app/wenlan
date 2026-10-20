// SPDX-License-Identifier: Apache-2.0
import { OAuthProvider, OAuthError, authorizationErrorRedirect, getOAuthApi, type AuthRequest, type OAuthHelpers, type OAuthProviderOptions } from '@cloudflare/workers-oauth-provider';
import { cleanupKV } from './cleanup-kv.ts';
import { beginPairing, cancelPairingRequest, consumePairing, type PairingStore } from './pairing.ts';
import { authorizationChallenge, isWriteGrant, isWriteToolCall, QUERY_SCOPE, readQueryBody, WRITE_SCOPE, type ConnectorRoute, type QueryGrant, type ProxyOptions } from './proxy.ts';
import { forwardSessionQuery } from './sessions.ts';
import { authorizationGrantActive, authorizationGrantEnd, claimAuthorizationGrant, replaceClientAuthorization, touchGrant } from './grants.ts';
import { randomSecret, validSecret } from './secrets.ts';

export interface OAuthEnv {
  OAUTH_KV: KVNamespace;
  OAUTH_PROVIDER: OAuthHelpers;
}
interface StoredAuthorization { request: AuthRequest; expiresAt: number }
const authorizationKey = (id: string) => `oauth-request:${id}`;
export { QUERY_SCOPE };
/** Scopes a grant may carry: the request is always query; write is the user's choice. */
const grantedScopes = (scope: readonly string[]) => scope.includes(QUERY_SCOPE) && new Set(scope).size === scope.length
  && scope.every(item => item === QUERY_SCOPE || item === WRITE_SCOPE);
const IDLE_REFRESH_SECONDS = 30 * 24 * 60 * 60;

function canonicalOrigin(value: string): string {
  const url = new URL(value);
  if (url.protocol !== 'https:' || url.origin !== value || url.username || url.password) {
    throw new Error('Invalid public OAuth origin');
  }
  return value;
}

/** The caller must deliver the browser secret only in a Secure/HttpOnly cookie,
 * and guard finish/cancel against CSRF. This function is not a public handler.
 */
export async function startOAuthPairing(
  oauth: OAuthHelpers, store: PairingStore, request: Request, publicOrigin: string,
) {
  const origin = canonicalOrigin(publicOrigin);
  const resource = `${origin}/mcp`;
  const parsed = await oauth.parseAuthRequest(request);
  if (parsed.responseType !== 'code' || parsed.issuer !== origin
    || parsed.resource !== resource || parsed.scope.length !== 1 || parsed.scope[0] !== QUERY_SCOPE
    || parsed.codeChallengeMethod !== 'S256'
    || !parsed.codeChallenge || !/^[A-Za-z0-9_-]{43}$/.test(parsed.codeChallenge)) {
    throw new Error('Unsupported OAuth authorization request');
  }
  const authorizationId = randomSecret();
  // The registered name is display-only context for the desktop app. Trust
  // and the browser's displayed name come from the validated redirect host.
  let clientName: string | null = null;
  try { clientName = (await oauth.lookupClient(parsed.clientId))?.clientName ?? null; } catch { clientName = null; }
  const pair = await beginPairing(store, {
    authorizationId, clientId: parsed.clientId, resource, scopes: parsed.scope,
  }, resource, Date.now(), { redirectUri: parsed.redirectUri, clientName });
  // No browser-visible state is returned until the validated request is durable.
  // Failure here leaves only an unusable pairing, never an issuable grant.
  await store.transaction(async tx => {
    await tx.put(authorizationKey(authorizationId), {
      request: structuredClone(parsed), expiresAt: pair.expiresAt,
    } satisfies StoredAuthorization);
  });
  return pair;
}

export async function finishOAuthPairing(
  oauth: OAuthHelpers, store: PairingStore, pairingId: string, browserSecret: string,
  publicOrigin: string,
): Promise<string | null> {
  const origin = canonicalOrigin(publicOrigin);
  const consumed = await consumePairing(store, pairingId, browserSecret);
  if (!consumed) return null;
  const saved = await store.transaction(tx => tx.get<StoredAuthorization>(authorizationKey(consumed.authorizationId)));
  if (!saved || !Number.isFinite(saved.expiresAt) || saved.expiresAt <= Date.now()
    || saved.request.clientId !== consumed.clientId || saved.request.issuer !== origin
    || saved.request.resource !== `${origin}/mcp` || consumed.resource !== `${origin}/mcp`
    || saved.request.scope.length !== 1 || saved.request.scope[0] !== QUERY_SCOPE) return null;
  await replaceClientAuthorization(store, consumed.grant.subject, consumed.clientId, consumed.authorizationId, consumed.client);
  const result = await oauth.completeAuthorization({
    request: saved.request, userId: consumed.grant.subject,
    metadata: { scope: consumed.grant.scopes.join(' ') }, scope: [...consumed.grant.scopes],
    props: { ...consumed.grant, authorizationId: consumed.authorizationId },
  });
  return result.redirectTo;
}

/** Cancels a browser pairing and returns the client's access_denied redirect,
 * built only from the validated, stored authorization request. Null when the
 * pairing is unavailable; '' when it was cancelled but no redirect is known.
 */
export async function cancelOAuthPairing(
  store: PairingStore, pairingId: string, browserSecret: string, publicOrigin: string,
): Promise<string | null> {
  const origin = canonicalOrigin(publicOrigin);
  const authorizationId = await cancelPairingRequest(store, pairingId, browserSecret);
  if (authorizationId === null) return null;
  const saved = await store.transaction(tx => tx.get<StoredAuthorization>(authorizationKey(authorizationId)));
  if (!saved || saved.request.issuer !== origin || typeof saved.request.redirectUri !== 'string') return '';
  try { return authorizationErrorRedirect(saved.request, 'access_denied'); } catch { return ''; }
}

const challenge = authorizationChallenge;
/** Internal: names the window that refused a request so the Worker can log it. Stripped before the response leaves. */
export const LIMIT_HEADER = 'x-wenlan-limit';

/** Must run behind OAuthProvider. The helper supplies the actual token's scope
 * and expiry, not the authorization-time props (which survive token refresh).
 */
export async function forwardOAuthQuery(
  request: Request, oauth: OAuthHelpers, publicOrigin: string,
  loadRoute: (id: string) => Promise<ConnectorRoute | null>,
  store: PairingStore,
  fetcher?: typeof globalThis.fetch,
  fetchReverse?: ProxyOptions['fetchReverse'],
  limits: { limitGrant?: (grantId: string) => Promise<number>; limitWrite?: (grantId: string) => Promise<number> } = {},
): Promise<Response> {
  const origin = canonicalOrigin(publicOrigin);
  const bearer = request.headers.get('authorization');
  if (!bearer?.startsWith('Bearer ') || bearer.length > 4096) return challenge(origin);
  let token;
  try { token = await oauth.unwrapToken<unknown>(bearer.slice(7)); }
  catch { return challenge(origin); }
  if (!token || token.audience !== `${origin}/mcp`
    || !Number.isFinite(token.expiresAt) || token.expiresAt * 1000 <= Date.now()
    || !Array.isArray(token.scope) || !token.scope.includes(QUERY_SCOPE)) return challenge(origin);
  const props = token.grant.props;
  if (!props || typeof props !== 'object') return challenge(origin);
  const bound = props as Partial<QueryGrant>;
  if (bound.subject !== token.userId || typeof bound.connectorId !== 'string'
    || !validSecret(bound.connectorId) || typeof bound.space !== 'string' || !bound.space.trim()
    || !Number.isSafeInteger(bound.generation) || (bound.generation ?? -1) < 0) return challenge(origin);
  const queryGrant: QueryGrant = {
    subject: token.userId, connectorId: bound.connectorId, space: bound.space,
    generation: bound.generation!, scopes: token.scope, expiresAt: token.expiresAt * 1000,
  };
  const identity = { grantId: token.grantId, clientId: token.grant.clientId };
  if (limits.limitGrant) {
    const retryAfter = await limits.limitGrant(token.grantId);
    if (retryAfter > 0) {
      return Response.json({ error: 'Request limit reached' }, { status: 429,
        headers: { 'cache-control': 'no-store', 'retry-after': String(retryAfter) } });
    }
  }
  // A write grant's tool calls outside the query set also spend a separate
  // hourly budget. Only the verified token's scope decides; a query grant's
  // write call is refused by the policy later and costs nothing here.
  if (limits.limitWrite && isWriteGrant(queryGrant) && request.method === 'POST'
    && isWriteToolCall(await readQueryBody(request.clone()).catch(() => new Uint8Array()))) {
    const retryAfter = await limits.limitWrite(token.grantId);
    if (retryAfter > 0) {
      return Response.json({ error: 'Request limit reached' }, { status: 429,
        headers: { 'cache-control': 'no-store', 'retry-after': String(retryAfter), [LIMIT_HEADER]: 'grant-write-hour' } });
    }
  }
  const authorize = async () => {
    if (!await authorizationGrantActive(store, identity, queryGrant)) return false;
    const current = await oauth.unwrapToken<unknown>(bearer.slice(7));
    return !!current && current.id === token.id && current.userId === token.userId
      && current.grantId === token.grantId && current.grant.clientId === token.grant.clientId
      && current.audience === `${origin}/mcp` && current.expiresAt * 1000 > Date.now()
      && current.scope.includes(QUERY_SCOPE);
  };
  const response = await forwardSessionQuery(request, queryGrant, identity, store,
    { publicOrigin: origin, loadRoute, fetch: fetcher, fetchReverse, authorize, grantActive: authorize });
  // Usage is display-only: a failed write never fails the authorized request.
  if (response.status < 400) {
    try { await touchGrant(store, identity, queryGrant); } catch { /* best effort */ }
  }
  return response;
}

/** DCR is the currently tested registration path. CIMD remains disabled until
 * the deployed runtime's strictly-public fetch flag and its flows are verified.
 */
export function createOAuthProvider<Env extends OAuthEnv>(
  publicOrigin: string, defaultHandler: ExportedHandler<Env>,
  apiHandler: { fetch(request: Request, env: Env, ctx: ExecutionContext): Promise<Response> },
  loadRoute: (id: string) => Promise<ConnectorRoute | null>,
  store: PairingStore,
) {
  const origin = canonicalOrigin(publicOrigin);
  const options: OAuthProviderOptions<Env> = {
    apiRoute: '/mcp', apiHandler, defaultHandler,
    authorizeEndpoint: `${origin}/authorize`, tokenEndpoint: `${origin}/oauth/token`,
    clientRegistrationEndpoint: `${origin}/oauth/register`,
    // A refresh token lapses after 30 idle days; each refresh slides it, capped
    // by the Wenlan consent and grant records (see the callback below).
    accessTokenTTL: 900, refreshTokenTTL: IDLE_REFRESH_SECONDS, refreshTokenIdleTTL: IDLE_REFRESH_SECONDS,
    clientRegistrationTTL: 90 * 24 * 60 * 60,
    scopesSupported: [QUERY_SCOPE], requiredScopes: [QUERY_SCOPE],
    allowTokenExchangeGrant: false, clientIdMetadataDocumentEnabled: false,
    resourceMetadata: { resource: `${origin}/mcp`, authorization_servers: [origin],
      bearer_methods_supported: ['header'], resource_name: 'Wenlan' },
    tokenExchangeCallback: async ({ props, userId, scope, requestedScope, grantType, grantId, clientId }) => {
      if (!grantedScopes(requestedScope) || !grantedScopes(scope)) {
        throw new OAuthError('invalid_scope', { description: 'Wenlan query permission is required' });
      }
      const bound = props as (Partial<QueryGrant> & { authorizationId?: string }) | null;
      if (!bound || typeof bound !== 'object' || bound.subject !== userId
        || typeof bound.connectorId !== 'string' || !validSecret(bound.connectorId)
        || !Number.isSafeInteger(bound.generation) || (bound.generation ?? -1) < 0
        || typeof bound.authorizationId !== 'string' || !validSecret(bound.authorizationId)) {
        throw new OAuthError('invalid_grant', { description: 'Wenlan authorization is invalid' });
      }
      const route = await loadRoute(bound.connectorId);
      if (!route || route.enabled !== true || route.id !== bound.connectorId
        || route.subject !== userId || route.space !== bound.space || route.generation !== bound.generation) {
        throw new OAuthError('invalid_grant', { description: 'Wenlan authorization has been revoked' });
      }
      const identity = { grantId, clientId };
      const grant = { subject: userId, connectorId: bound.connectorId, space: route.space, generation: route.generation, scopes: scope };
      // The library revokes the grant on invalid_grant, so only permanent
      // denials may use it; storage failures surface as server errors instead.
      const invalid = () => new OAuthError('invalid_grant', { description: 'Wenlan authorization is no longer valid' });
      if (grantType === 'authorization_code') {
        if (!await claimAuthorizationGrant(store, identity, grant, bound.authorizationId)) throw invalid();
        // The user's choice outlives a client asking for a narrower scope.
        return { accessTokenScope: [...scope] };
      }
      const end = await authorizationGrantEnd(store, identity, grant);
      const remaining = end === null ? 0 : Math.floor((end - Date.now()) / 1000);
      // The provider refuses idle TTLs under 60 seconds; the grant is spent by then.
      if (remaining < 60) throw invalid();
      return { refreshTokenIdleTTL: Math.min(IDLE_REFRESH_SECONDS, remaining), accessTokenScope: [...scope] };
    },
    // Avoid the library's default warning logs containing request error details.
    onError: () => {},
  };
  const provider = new OAuthProvider<Env>(options);
  return {
    fetch: (request: Request, env: Env, ctx: ExecutionContext) => provider.fetch(request, env, ctx),
    // Alarms after eviction have no prior fetch to populate env.OAUTH_PROVIDER.
    revokeStoredGrant: (env: Env, grantId: string, subject: string) => getOAuthApi(options,
      { ...env, OAUTH_KV: cleanupKV(env.OAUTH_KV) }).revokeGrant(grantId, subject),
  };
}
