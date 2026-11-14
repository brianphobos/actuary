#!/usr/bin/env node
/*
 * Actuary Hive oracle (reference).
 *   node verifier/hive-oracle.js <near-verifier-account> <hive-account> <keychain-signature-hex> [--contract <account>] [--oracle <account>]
 *
 * The verifier signs "actuary:verifier:<near account>" with Hive Keychain (the dashboard's
 * "Become a verifier" page does this and shows the signature). This script checks that the
 * signature is from one of the Hive account's posting or active keys, reads its reputation
 * from a public Hive API node, and prints the near-cli command that records the result on
 * chain. The contract approves the verifier only if the score is at least 55.
 */
const H = require('../sim/hive.js');
const [near, hive, sig] = process.argv.slice(2).filter((a, i, all) => !a.startsWith('--') && !(all[i - 1] || '').startsWith('--'));
const opt = (k, d) => { const i = process.argv.indexOf(k); return i >= 0 ? process.argv[i + 1] : d; };
if (!near || !hive || !sig) { console.error('usage: node verifier/hive-oracle.js <near-verifier-account> <hive-account> <signature-hex>'); process.exit(2); }
(async () => {
  const r = await H.checkVerifier(near, hive, sig);
  console.log(`@${hive} for ${near}: ${r.ok ? 'APPROVE' : 'REJECT'} (${r.reason})${r.reputation != null ? `, reputation ${r.reputation.toFixed(2)}` : ''}`);
  if (r.reputation == null || !r.key) process.exit(1);
  const contract = opt('--contract', 'actuary.testnet'), oracle = opt('--oracle', 'hive-oracle.testnet');
  const args = JSON.stringify({ verifier: near, hive_account: hive, reputation_x100: Math.round(r.reputation * 100) });
  console.log(`near contract call-function as-transaction ${contract} attest_hive json-args '${args}' prepaid-gas '30.0 Tgas' attached-deposit '0 NEAR' sign-as ${oracle} network-config testnet sign-with-keychain send`);
})().catch((e) => { console.error(e.message); process.exit(1); });
