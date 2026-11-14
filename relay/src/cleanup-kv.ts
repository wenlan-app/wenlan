// SPDX-License-Identifier: Apache-2.0

export class CleanupBatchIncomplete extends Error {
  constructor() { super('OAuth cleanup batch incomplete'); }
}

/** Physical cleanup of an invalidated grant: one KV delete of the provider's
 * grant record, and no KV list. The free Workers plan allows only 1,000 KV list
 * operations a day, and the provider's revokeGrant helper lists the grant's
 * access-token records every time it runs. Those records need no deletion: the
 * Wenlan receipt already denies them before any backend call, and they expire on
 * their own after the 15-minute access-token TTL. Deleting the grant record ends
 * refresh at the provider as well.
 */
export async function deleteProviderGrant(kv: KVNamespace, grantId: string, subject: string): Promise<void> {
  if (!grantId || !subject || grantId.includes(':') || subject.includes(':')) throw new Error('Invalid OAuth grant');
  await kv.delete(`grant:${subject}:${grantId}`);
}
