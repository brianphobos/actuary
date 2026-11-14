/*
 * Hive identity for Actuary verifiers. No dependencies; Node 18+ and browsers.
 *
 *  - reputationScore(raw): Hive's raw reputation -> the familiar score (25 for a new account).
 *  - fetchAccount(name): reads the account from a public Hive API node.
 *  - verifySignature(message, sigHex, pubKeyStr): checks a Hive Keychain `requestSignBuffer`
 *    signature (secp256k1 over sha256(message)) against an STM... public key.
 *  - checkVerifier(nearAccount, hiveAccount, sigHex): the whole oracle check: the signature
 *    on "actuary:verifier:<near account>" is from one of the account's posting/active keys,
 *    and the reputation score is at least 55.
 *
 * The NEAR contract cannot read Hive, so a Hive oracle account runs this check and calls
 * `attest_hive` on chain. The dashboard runs the same check in the browser first.
 */
(function (root, factory) {
  const api = factory();
  if (typeof module === 'object' && module.exports) module.exports = api;
  else root.ActuaryHive = api;
})(typeof self !== 'undefined' ? self : this, function () {
  'use strict';
  const MIN_REP = 55;
  const NODES = ['https://api.hive.blog', 'https://api.deathwing.me', 'https://anyx.io'];
  const challenge = (nearAccount) => `actuary:verifier:${nearAccount}`;

  function reputationScore(raw) {
    let r = typeof raw === 'string' ? Number(raw) : raw;
    if (!r) return 25;
    const neg = r < 0;
    let level = Math.log10(Math.abs(r));
    level = Math.max(level - 9, 0) * (neg ? -1 : 1);
    return Math.floor((level * 9 + 25) * 100) / 100;
  }

  async function fetchAccount(name, fetchImpl = globalThis.fetch) {
    let lastErr;
    for (const node of NODES) {
      try {
        const res = await fetchImpl(node, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ jsonrpc: '2.0', method: 'condenser_api.get_accounts', params: [[name]], id: 1 }) });
        const j = await res.json();
        if (j.error) throw new Error(j.error.message);
        return j.result[0] || null;
      } catch (e) { lastErr = e; }
    }
    throw lastErr || new Error('no Hive API node reachable');
  }

  // ---- secp256k1 (verify only) ----
  const P = 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffefffffc2fn;
  const N = 0xfffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364141n;
  const G = [0x79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798n, 0x483ada7726a3c4655da4fbfc0e1108a8fd17b448a68554199c47d08ffb10d4b8n];
  const mod = (a, m = P) => ((a % m) + m) % m;
  function inv(a, m = P) { let [x, y, u, v] = [mod(a, m), m, 1n, 0n]; while (x) { const q = y / x; [x, y] = [y - q * x, x]; [u, v] = [v - q * u, u]; } return mod(v, m); }
  function add(p, q) {
    if (!p) return q; if (!q) return p;
    if (p[0] === q[0]) { if (mod(p[1] + q[1]) === 0n) return null; const l = mod(3n * p[0] * p[0] * inv(2n * p[1])); const x = mod(l * l - 2n * p[0]); return [x, mod(l * (p[0] - x) - p[1])]; }
    const l = mod((q[1] - p[1]) * inv(q[0] - p[0])); const x = mod(l * l - p[0] - q[0]); return [x, mod(l * (p[0] - x) - p[1])];
  }
  function mul(k, p) { let r = null; for (; k > 0n; k >>= 1n, p = add(p, p)) if (k & 1n) r = add(r, p); return r; }
  function powMod(b, e, m) { let r = 1n; b = mod(b, m); for (; e > 0n; e >>= 1n, b = mod(b * b, m)) if (e & 1n) r = mod(r * b, m); return r; }
  function decompress(bytes) {
    const x = BigInt('0x' + hex(bytes.slice(1)));
    let y = powMod(mod(x * x * x + 7n), (P + 1n) / 4n, P);
    if ((y & 1n) !== BigInt(bytes[0] & 1)) y = P - y;
    return [x, y];
  }
  const hex = (b) => Array.from(b, (x) => x.toString(16).padStart(2, '0')).join('');
  const fromHex = (h) => new Uint8Array(h.match(/../g).map((x) => parseInt(x, 16)));
  const B58 = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';
  function b58decode(s) {
    let n = 0n; for (const c of s) { const i = B58.indexOf(c); if (i < 0) throw new Error('bad base58'); n = n * 58n + BigInt(i); }
    let h = n.toString(16); if (h.length % 2) h = '0' + h;
    const lead = s.match(/^1*/)[0].length;
    return new Uint8Array([...new Array(lead).fill(0), ...fromHex(h)]);
  }
  function b58encode(bytes) {
    let n = BigInt('0x' + (hex(bytes) || '0')), s = '';
    while (n > 0n) { s = B58[Number(n % 58n)] + s; n /= 58n; }
    for (const b of bytes) { if (b) break; s = '1' + s; }
    return s;
  }
  /** "STM..." -> 33-byte compressed key (the 4-byte ripemd160 checksum is dropped, not checked). */
  function parsePublicKey(str) {
    const body = str.replace(/^(STM|TST)/, '');
    const raw = b58decode(body);
    if (raw.length !== 37) throw new Error('not a Hive public key');
    return raw.slice(0, 33);
  }
  async function sha256(msg) {
    const data = typeof msg === 'string' ? new TextEncoder().encode(msg) : msg;
    return new Uint8Array(await globalThis.crypto.subtle.digest('SHA-256', data));
  }
  /** Verify a 65-byte Hive signature (hex: recovery byte, r, s) over sha256(message). */
  async function verifySignature(message, sigHex, pubKeyStr) {
    const sig = fromHex(sigHex);
    if (sig.length !== 65) return false;
    const r = BigInt('0x' + hex(sig.slice(1, 33))), s = BigInt('0x' + hex(sig.slice(33)));
    if (r <= 0n || r >= N || s <= 0n || s >= N) return false;
    const e = BigInt('0x' + hex(await sha256(message)));
    const Q = decompress(parsePublicKey(pubKeyStr));
    const w = inv(s, N);
    const X = add(mul(mod(e * w, N), G), mul(mod(r * w, N), Q));
    return !!X && mod(X[0], N) === r;
  }

  /** The oracle's check. Returns { ok, reason, reputation, key }. */
  async function checkVerifier(nearAccount, hiveAccount, sigHex, opts = {}) {
    const acct = opts.account || (await fetchAccount(hiveAccount, opts.fetch));
    if (!acct) return { ok: false, reason: `@${hiveAccount} does not exist on Hive` };
    const reputation = reputationScore(acct.reputation);
    const keys = [...acct.posting.key_auths, ...acct.active.key_auths].map((k) => k[0]);
    let key = null;
    for (const k of keys) if (await verifySignature(challenge(nearAccount), sigHex, k)) { key = k; break; }
    if (!key) return { ok: false, reason: 'signature is not from this Hive account', reputation };
    if (reputation < MIN_REP) return { ok: false, reason: `Hive reputation ${reputation.toFixed(2)} is below ${MIN_REP}`, reputation, key };
    return { ok: true, reason: 'signed by the account, reputation high enough', reputation, key };
  }

  return { MIN_REP, challenge, reputationScore, fetchAccount, verifySignature, checkVerifier, parsePublicKey, b58encode, b58decode, hex, fromHex };
});
