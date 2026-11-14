#!/usr/bin/env node
// Dry-run CLI. No wallet, no network, no funds.
//   node sim/run.js --story          walk through the scripted demo
//   node sim/run.js --ticks 600      run the swarm market for 600 simulated hours
//   node sim/run.js --json           print the swarm report as JSON
const fs = require('node:fs');
const path = require('node:path');
const { createSwarm, createStory } = require('./swarm.js');

const args = process.argv.slice(2);
const opt = (k, d) => { const i = args.indexOf(k); return i >= 0 ? args[i + 1] : d; };
const has = (k) => args.includes(k);
const c = { b: '\x1b[1m', d: '\x1b[2m', g: '\x1b[32m', r: '\x1b[31m', y: '\x1b[33m', x: '\x1b[0m' };

if (has('--story')) {
  const s = createStory();
  console.log(`${c.b}Actuary: the insurance market for AI agent work (story mode, dry run)${c.x}\n`);
  s.steps.forEach((st, i) => {
    const text = st.run();
    console.log(`${c.y}${String(i + 1).padStart(2)}. ${st.title}${c.x}\n    ${text}\n`);
  });
  process.exit(0);
}

const ticks = Number(opt('--ticks', 600));
const seed = Number(opt('--seed', 7));
const sw = createSwarm({ seed });
for (let i = 0; i < ticks; i++) sw.step();
const r = sw.report();
fs.mkdirSync(path.join(__dirname, 'out'), { recursive: true });
fs.writeFileSync(path.join(__dirname, 'out', 'report.json'), JSON.stringify(r, null, 2));
if (has('--json')) { console.log(JSON.stringify(r, null, 2)); process.exit(0); }

const pct = (x) => (x * 100).toFixed(1) + '%';
const nr = (x) => x.toFixed(2);
console.log(`${c.b}Actuary swarm simulation${c.x} ${c.d}(seed ${seed}, ${ticks} simulated hours, dry run)${c.x}\n`);
console.log(`Jobs: ${r.jobsPosted} posted, ${r.jobsResolved} resolved, pass rate ${pct(r.passRate)}`);
console.log(`\n${c.b}Does the market know who will deliver?${c.x}`);
console.log(`  Brier score, opening price from track record only : ${r.brierOpeningPrior.toFixed(3)}`);
console.log(`  Brier score, market price at close               : ${c.g}${r.brierMarketClose.toFixed(3)}${c.x}  (lower is better)`);
console.log('  Calibration (market close price vs what happened):');
for (const b of r.calibration) if (b.n) console.log(`    priced ${pct(b.lo).padStart(6)}-${pct(b.hi).padEnd(6)}  n=${String(b.n).padStart(3)}  predicted ${pct(b.predicted).padStart(6)}  actual ${pct(b.actual).padStart(6)}`);
console.log(`\n${c.b}Buyers${c.x}`);
console.log(`  Consequential losses from failed jobs : ${nr(r.buyers.consequentialLoss)} NEAR`);
console.log(`  Paid out by cover                     : ${c.g}${nr(r.buyers.coverPaid)} NEAR${c.x}`);
console.log(`  Premiums paid                         : ${nr(r.buyers.premiums)} NEAR`);
console.log(`  Losses left uninsured (no cover bought): ${nr(r.buyers.uninsuredLoss)} NEAR`);
console.log(`\n${c.b}Underwriting vault${c.x}`);
console.log(`  ${r.vault.start} NEAR -> ${nr(r.vault.assets)} NEAR (${r.vault.returnPct >= 0 ? c.g : c.r}${r.vault.returnPct.toFixed(2)}%${c.x}), of which fees ${nr(r.vault.fees)} NEAR`);
console.log(`\n${c.b}Agents${c.x}  ${c.d}(record, next opening price, earned, bond slashed)${c.x}`);
for (const a of r.agents) console.log(`  ${a.id.padEnd(15)} ${a.kind.padEnd(8)} ${String(a.passes).padStart(3)} / ${String(a.fails).padEnd(3)} next ${pct(a.prior).padStart(6)}  earned ${nr(a.earned).padStart(7)}  slashed ${nr(a.slashed).padStart(6)}${a.kind === 'rookie' ? `  first hired at hour ${a.hiredAt} (joined ${a.joins})` : ''}`);
console.log(`\n${c.b}Verifiers${c.x}  ${c.d}(Hive reputation, status, fee, rulings, missed, fees earned, pledge lost)${c.x}`);
for (const v of r.verifiers) console.log(`  ${v.id.padEnd(15)} @${v.hive.padEnd(9)} rep ${v.rep.toFixed(1).padStart(5)}  ${(v.status === 'Approved' ? c.g : c.r) + v.status.padEnd(9) + c.x} fee ${(v.feeBps / 100).toFixed(1).padStart(4)}%  ${String(v.rulings).padStart(4)} rulings  ${String(v.timeouts).padStart(2)} missed  earned ${nr(v.earned).padStart(6)}  lost ${nr(v.slashed).padStart(6)}`);
console.log(`\n${c.b}Underwriter swarm P&L${c.x}`);
for (const t of r.traders) console.log(`  ${t.id.padEnd(18)} ${t.kind.padEnd(8)} ${t.pnl >= 0 ? c.g : c.r}${nr(t.pnl).padStart(8)} NEAR${c.x}`);
if (r.sleeper.turnedOnJob !== null) {
  console.log(`\n${c.b}Reputation-farming attack${c.x}: sleeper.agent turned on job #${r.sleeper.turnedOnJob}.`);
  for (const s of r.sleeper.closes.slice(0, 5)) console.log(`  job #${s.job}: opened at ${pct(s.prior)} on its record, market closed at ${c.r}${pct(s.close)}${c.x} before it failed`);
}
if (r.rejections.length) {
  console.log(`\n${c.b}Refused by the contract${c.x}`);
  for (const x of r.rejections.slice(0, 5)) console.log(`  job #${x.job} ${x.who}: ${x.msg}`);
}
console.log(`\n${c.d}Full report: sim/out/report.json${c.x}`);
