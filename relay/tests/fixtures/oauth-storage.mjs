// SPDX-License-Identifier: Apache-2.0
export async function oauthKV(runtime, name = 'wenlan-v1') {
  const ns = await runtime.getDurableObjectNamespace('AUTHORITY');
  const stub = ns.get(ns.idFromName(name));
  return Object.fromEntries(['get', 'put', 'delete', 'list'].map(operation => [operation, async (...args) => {
    const response = await stub.fetch('https://wenlan-relay.example/__oauth_storage', {
      method: 'POST', body: JSON.stringify({ operation, args }),
    });
    if (!response.ok) throw new Error(`OAuth fixture ${operation}: ${await response.text()}`);
    return response.json();
  }]));
}
