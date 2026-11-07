// SPDX-License-Identifier: AGPL-3.0-only
export function safePublicSourceUrl(raw?: string): string | null {
  if (!raw) return null;
  try {
    const url = new URL(raw);
    const host = url.hostname.toLowerCase().replace(/\.$/, "");
    if (!["http:", "https:"].includes(url.protocol) || url.username || url.password) return null;
    const blockedKeys = new Set(["auth", "authorization", "password", "passwd", "secret", "api_key", "apikey", "sig", "signature", "x_amz_credential", "x_goog_credential", "x_amz_signature", "x_goog_signature"]);
    for (const key of [...url.searchParams.keys(), ...new URLSearchParams(url.hash.slice(1)).keys()]) {
      const normalized = key.toLowerCase().replace(/-/g, "_");
      if (blockedKeys.has(normalized) || normalized === "token" || normalized.endsWith("_token")) return null;
    }
    if (host.startsWith("[")) {
      const address = host.slice(1, -1);
      const first = parseInt(address.split(":")[0], 16);
      if (address.startsWith("::") || (first & 0xfe00) === 0xfc00 || (first & 0xffc0) === 0xfe80 || (first & 0xff00) === 0xff00) return null;
    } else if (/^\d+(?:\.\d+){3}$/.test(host)) {
      const [a, b] = host.split(".").map(Number);
      if (a === 0 || a === 10 || a === 127 || a >= 224 || (a === 169 && b === 254) ||
          (a === 172 && b >= 16 && b <= 31) || (a === 192 && b === 168) || (a === 100 && b >= 64 && b <= 127)) return null;
    } else if (!host.includes(".") || ["localhost", "local", "internal", "lan", "home", "test", "invalid"].some(suffix => host === suffix || host.endsWith(`.${suffix}`))) return null;
    return url.href;
  } catch { return null; }
}

