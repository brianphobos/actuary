#!/usr/bin/env node
/*
 * Actuary reference verifier.
 *   node verifier/verify.js <spec-dir> <deliverable.js> [--job <id>] [--contract <account>]
 *
 * Runs the job's acceptance cases against the delivered code in a separate
 * process with a time limit, then prints the verdict, an evidence hash that
 * binds spec + deliverable + results together, and the exact near-cli command
 * the verifier account would sign to resolve the job.
 *
 * In production this runs inside a TEE (OutLayer or NEAR AI Cloud) so the
 * evidence hash comes with a hardware attestation. Here it runs locally.
 */
const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');
const { spawnSync } = require('node:child_process');

const [specDir, deliverable] = process.argv.slice(2).filter((a, i, all) => !a.startsWith('--') && !(all[i - 1] || '').startsWith('--'));
const opt = (k, d) => { const i = process.argv.indexOf(k); return i >= 0 ? process.argv[i + 1] : d; };
if (!specDir || !deliverable) { console.error('usage: node verifier/verify.js <spec-dir> <deliverable.js> [--job <id>] [--contract <account>]'); process.exit(2); }

const cases = JSON.parse(fs.readFileSync(path.join(specDir, 'cases.json'), 'utf8'));
const runner = `
  const { slugify } = require(${JSON.stringify(path.resolve(deliverable))});
  const cases = ${JSON.stringify(cases)};
  const out = cases.map((c) => { try { const got = slugify(c.input); return { ok: got === c.expected, got }; } catch (e) { return { ok: false, got: 'threw: ' + e.message }; } });
  process.stdout.write(JSON.stringify(out));
`;
const r = spawnSync(process.execPath, ['-e', runner], { encoding: 'utf8', timeout: 10_000 });
let results;
try { results = JSON.parse(r.stdout); } catch { results = cases.map(() => ({ ok: false, got: 'did not run: ' + (r.stderr || r.error || 'timeout').toString().slice(0, 200) })); }

const passed = results.filter((x) => x.ok).length;
const outcome = passed === cases.length ? 'Pass' : 'Fail';
const h = crypto.createHash('sha256');
for (const f of ['spec.md', 'cases.json']) h.update(fs.readFileSync(path.join(specDir, f)));
h.update(fs.readFileSync(deliverable)); h.update(JSON.stringify(results));
const evidence = `sha256:${h.digest('hex').slice(0, 32)} tests ${passed}/${cases.length}`;

results.forEach((x, i) => { if (!x.ok) console.log(`  ✗ case ${i + 1}: ${JSON.stringify(cases[i].input).slice(0, 40)} → got ${JSON.stringify(x.got).slice(0, 40)}, expected ${JSON.stringify(cases[i].expected).slice(0, 40)}`); });
console.log(`\nverdict : ${outcome.toUpperCase()} (${passed}/${cases.length} cases)`);
console.log(`evidence: ${evidence}`);
const job = opt('--job', '<JOB_ID>'), contract = opt('--contract', '<CONTRACT>');
console.log(`\nresolve on-chain (signed by the verifier account):\n  near contract call-function as-transaction ${contract} resolve json-args '${JSON.stringify({ job_id: Number(job) || 0, outcome, evidence })}' prepaid-gas '30 Tgas' attached-deposit '0 NEAR' sign-as <VERIFIER_ACCOUNT> network-config testnet sign-with-keychain send`);
process.exit(outcome === 'Pass' ? 0 : 1);
