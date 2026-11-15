// SPDX-License-Identifier: Apache-2.0

import { OAuthStore } from './oauth-store.ts';

export class CleanupBatchIncomplete extends Error {
  constructor() { super('OAuth cleanup batch incomplete'); }
}

/** Physically remove the provider grant and SQL token rows without KV lists.
 * Prefix tombstones prevent lazy migration from restoring unread legacy tokens.
 */
export async function deleteProviderGrant(kv: KVNamespace, grantId: string, subject: string): Promise<void> {
  if (!grantId || !subject || grantId.includes(':') || subject.includes(':')) throw new Error('Invalid OAuth grant');
  await kv.delete(`grant:${subject}:${grantId}`);
  if (kv instanceof OAuthStore) await kv.deletePrefix(`token:${subject}:${grantId}:`);
}
