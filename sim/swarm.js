/*
 * Actuary swarm: scripted AI agents trading on the Actuary engine.
 * No LLM calls and no randomness beyond a seeded RNG, so every run is reproducible.
 *
 *  - Worker agents take jobs. Each has a hidden skill; some are honest, one farms
 *    reputation and then exit-scams, one is a fraud, two are rookies who join late.
 *  - Buyers post jobs, escrow payment, and buy cover (FAIL shares) for their
 *    consequential loss if the work fails.
 *  - The underwriter swarm prices risk: actuaries read agents' on-chain records,
 *    watchers read job heartbeats (progress signals), noise traders add flow.
 *  - A verifier (standing in for a TEE that runs the job's test suite) resolves jobs.
 */
(function (root, factory) {
  const api = factory(typeof module === 'object' && module.exports ? require('./engine.js') : root.ActuaryEngine);
  if (typeof module === 'object' && module.exports) module.exports = api;
  else root.ActuarySwarm = api;
})(typeof self !== 'undefined' ? self : this, function (E) {
  'use strict';
  const { NEAR, HOUR, Actuary } = E;
  const n = (x) => BigInt(Math.round(x * 1e6)) * 10n ** 18n; // NEAR float -> yocto
  const f = (y) => Number(y / 10n ** 18n) / 1e6; // yocto -> NEAR float

  function rng(seed) {
    let a = seed >>> 0;
    return () => { a = (a + 0x6d2b79f5) >>> 0; let t = a; t = Math.imul(t ^ (t >>> 15), t | 1); t ^= t + Math.imul(t ^ (t >>> 7), t | 61); return ((t ^ (t >>> 14)) >>> 0) / 4294967296; };
  }

  // P(heartbeat on time | job will pass) and P(heartbeat | job will fail)
  const HB_PASS = 0.92, HB_FAIL = 0.3;

  const WORKERS = [
    { id: 'atlas.agent', skill: 0.95, bondFrac: 0.12, joins: 0, kind: 'veteran' },
    { id: 'orion.agent', skill: 0.9, bondFrac: 0.12, joins: 0, kind: 'veteran' },
    { id: 'quill.agent', skill: 0.82, bondFrac: 0.15, joins: 0, kind: 'steady' },
    { id: 'flux.agent', skill: 0.62, bondFrac: 0.12, joins: 0, kind: 'flaky' },
    { id: 'sleeper.agent', skill: 0.97, bondFrac: 0.12, joins: 0, kind: 'sleeper', turnsAfter: 10 },
    { id: 'ghost.agent', skill: 0.2, bondFrac: 0.1, joins: 0, kind: 'fraud' },
    { id: 'nova.agent', skill: 0.93, bondFrac: 0.4, joins: 120, kind: 'rookie' },
    { id: 'mint.agent', skill: 0.88, bondFrac: 0.35, joins: 200, kind: 'rookie' },
  ];
  const BUYERS = ['acme.buyer', 'globex.buyer', 'initech.buyer', 'umbrella.buyer', 'hooli.buyer', 'stark.buyer'];
  const TRADERS = [
    ...['actuary-1', 'actuary-2', 'actuary-3'].map((id) => ({ id: id + '.swarm', kind: 'actuary' })),
    ...['watcher-1', 'watcher-2', 'watcher-3', 'watcher-4'].map((id) => ({ id: id + '.swarm', kind: 'watcher' })),
    ...['noise-1', 'noise-2'].map((id) => ({ id: id + '.swarm', kind: 'noise' })),
  ];
  const VERIFIER = 'verifier.tee';

  /** Trade in small steps toward a belief, stopping when the edge is below the fee. */
  function tradeToward(m, who, jobId, belief, maxSpend, step, tag) {
    let spent = 0, trades = 0;
    while (spent + step <= maxSpend) {
      const job = m.job(jobId); if (job.status !== 'Active') break;
      const p = m.passPriceBps(jobId) / 10000;
      const edge = belief - p;
      if (Math.abs(edge) < 0.03) break;
      const outcome = edge > 0 ? 'Pass' : 'Fail';
      if (who === job.agent && outcome === 'Fail') break;
      const q = m.quote(jobId, outcome, n(step));
      const after = q.passPriceAfterBps / 10000;
      if ((edge > 0 && after > belief) || (edge < 0 && after < belief)) break;
      m.buy(who, jobId, outcome, n(step), 0n, tag); spent += step; trades++;
    }
    return trades;
  }

  function createSwarm({ seed = 7, feeBps = 300, takeBps = 200, liquidityBps = 10000, vault = 3000 } = {}) {
    const R = rng(seed);
    const m = new Actuary({ feeBps, takeBps, liquidityBps });
    m.vaultDeposit('lp-alpha.near', n(vault * 0.6));
    m.vaultDeposit('lp-beta.near', n(vault * 0.4));
    const vaultStart = vault;
    const live = new Map(); // jobId -> hidden info
    const workerState = new Map(WORKERS.map((w) => [w.id, { ...w, busy: 0, hiredAt: null }]));
    const stats = { ticks: 0, consequentialLoss: 0, coverPaid: 0, premiums: 0, uninsuredLoss: 0, insuredLossAfterCover: 0,
      rejections: [], closes: [], vaultCurve: [], rookieHires: {}, sleeperTurned: null, posted: 0, cancelled: 0 };
    let tick = 0;

    function pickAgent() {
      const avail = [...workerState.values()].filter((w) => w.joins <= tick && w.busy < 2);
      if (!avail.length) return null;
      // Buyers look at the market's opening price for each agent, plus what the agent offers to bond.
      const scores = avail.map((w) => { const p = m.agentPriorBps(w.id) / 10000; return Math.pow(p, 3) * (1 + 1.2 * w.bondFrac); });
      let r = R() * scores.reduce((a, b) => a + b, 0);
      for (let i = 0; i < avail.length; i++) { r -= scores[i]; if (r <= 0) return avail[i]; }
      return avail[avail.length - 1];
    }

    function postOne() {
      const w = pickAgent(); if (!w) return;
      const buyer = BUYERS[Math.floor(R() * BUYERS.length)];
      const pay = Math.round((2 + R() * 18) * 10) / 10;
      const dur = 8 + Math.floor(R() * 32);
      m.now = BigInt(tick) * HOUR;
      const id = m.postJob(buyer, { agent: w.id, verifier: VERIFIER, spec: `sha256:spec-${tick}`, deadline: m.now + BigInt(dur) * HOUR, grace: 2n * HOUR }, n(pay));
      stats.posted++;
      const rec = m.getAgent(w.id);
      let skill = w.skill;
      if (w.kind === 'sleeper' && rec.passes >= w.turnsAfter) { skill = 0.03; if (stats.sleeperTurned === null) stats.sleeperTurned = id; }
      const willPass = R() < skill * (0.92 + R() * 0.08);
      try {
        m.acceptJob(w.id, id, n(pay * w.bondFrac));
      } catch (e) { stats.cancelled++; stats.rejections.push({ job: id, who: w.id, msg: e.message }); return; }
      w.busy++;
      if (w.kind === 'rookie' && w.hiredAt === null) { w.hiredAt = tick; stats.rookieHires[w.id] = tick; }
      // Buyer buys cover for consequential loss (missed launch, downtime): 0-100% of the payment.
      const loss = pay * (0.6 + R() * 0.8);
      const coverFrac = R() < 0.25 ? 0 : 0.5 + R() * 0.5;
      let premium = 0, cover = 0n;
      if (coverFrac > 0) {
        const want = n(loss * coverFrac);
        const cost = m.costFor(id, 'Fail', want);
        if (f(cost) < loss * coverFrac * 0.6) { m.buy(buyer, id, 'Fail', cost, 0n, 'cover'); premium = f(cost); cover = m.getPosition(id, buyer).fail; }
      }
      // The agent sometimes tries to short itself (the contract refuses).
      if (w.kind === 'sleeper' && !willPass) {
        try { m.buy(w.id, id, 'Fail', n(5)); } catch (e) { stats.rejections.push({ job: id, who: w.id, msg: e.message }); }
      }
      const hbAt = [0.25, 0.5, 0.75].map((x) => tick + Math.max(1, Math.round(dur * x)));
      live.set(id, { id, worker: w.id, willPass, end: tick + dur, hbAt, hbSeen: [], loss, premium, cover, buyer, silent: !willPass && R() < 0.5 });
      // Actuaries price the open from the agent's public record.
      for (const t of TRADERS.filter((t) => t.kind === 'actuary')) {
        const r = m.getAgent(w.id);
        const a = r.passes + 1, b = r.fails + 1;
        const belief = Math.min(0.97, Math.max(0.03, a / (a + b) + (R() - 0.5) * 0.06));
        tradeToward(m, t.id, id, belief, 3, 0.25, 'actuary');
      }
    }

    function step() {
      tick++; m.now = BigInt(tick) * HOUR; stats.ticks = tick;
      // new jobs
      const arrivals = R() < 0.7 ? 1 : 0;
      for (let i = 0; i < arrivals; i++) postOne();
      for (const L of [...live.values()]) {
        const job = m.job(L.id);
        // heartbeats
        L.hbAt.forEach((t, i) => {
          if (t !== tick) return;
          const hit = R() < (L.willPass ? HB_PASS : HB_FAIL);
          L.hbSeen.push(hit);
          // The verifier attests the heartbeat; the market maker reprices itself.
          if (job.status === 'Active' && m.now < job.deadline) m.heartbeat(VERIFIER, L.id, hit, hit ? 'sha256:progress' : 'missed');
          // Watchers have their own, noisier read of the work (they look at the commits),
          // and trade only when it disagrees with the price.
          for (const t of TRADERS.filter((t) => t.kind === 'watcher')) {
            if (R() < 0.5 || job.status !== 'Active' || m.now >= job.deadline) continue;
            const p = m.passPriceBps(L.id) / 10000;
            const right = R() < 0.7; // 70% of the time their private read is correct
            const says = right ? L.willPass : !L.willPass;
            const lr = says ? 1.6 : 1 / 1.6;
            const odds = (p / (1 - p)) * lr;
            tradeToward(m, t.id, L.id, Math.min(0.97, Math.max(0.03, odds / (1 + odds))), 3, 0.3, 'watcher');
          }
        });
        // noise
        for (const t of TRADERS.filter((t) => t.kind === 'noise')) {
          if (R() < 0.04 && job.status === 'Active' && m.now < job.deadline) m.buy(t.id, L.id, R() < 0.5 ? 'Pass' : 'Fail', n(0.1 + R() * 0.4), 0n, 'noise');
        }
        // resolution
        if (tick === L.end && !(L.silent)) m.resolve(VERIFIER, L.id, L.willPass ? 'Pass' : 'Fail', L.willPass ? 'tests 24/24 passed' : 'tests 9/24 passed');
        if (L.silent && m.now >= job.deadline + job.grace && job.status === 'Active') m.expire(L.id);
        if (job.status === 'Passed' || job.status === 'Failed') finish(L, job);
      }
      stats.vaultCurve.push({ t: tick, assets: f(m.getVault().totalAssets) });
    }

    function finish(L, job) {
      live.delete(L.id);
      workerState.get(L.worker).busy--;
      const passed = job.status === 'Passed';
      stats.closes.push({ job: L.id, agent: L.worker, prior: job.priorBps / 10000, open: (job.history[1] || job.history[0]).p / 10000,
        close: job.closingBps / 10000, passed, premium: L.premium, cover: f(L.cover), loss: L.loss, heartbeats: L.hbSeen });
      if (!passed) {
        stats.consequentialLoss += L.loss;
        if (L.cover > 0n) { stats.coverPaid += f(L.cover); stats.insuredLossAfterCover += Math.max(0, L.loss - f(L.cover)); }
        else stats.uninsuredLoss += L.loss;
      }
      stats.premiums += L.premium;
      // everyone holding winning shares redeems
      for (const [key, pos] of [...m.positions.entries()]) {
        if (!key.startsWith(L.id + ':')) continue;
        const who = key.slice(String(L.id).length + 1);
        if ((passed ? pos.pass : pos.fail) > 0n) m.redeem(who, L.id); else m.positions.delete(key);
      }
    }

    function report() {
      const brier = (k) => stats.closes.reduce((s, c) => s + (c[k] - (c.passed ? 1 : 0)) ** 2, 0) / Math.max(1, stats.closes.length);
      const bins = [0, 0.2, 0.4, 0.6, 0.8, 1.0001].slice(0, -1).map((lo, i, a) => {
        const hi = [0.2, 0.4, 0.6, 0.8, 1.0001][i];
        const xs = stats.closes.filter((c) => c.close >= lo && c.close < hi);
        return { lo, hi: Math.min(1, hi), n: xs.length, predicted: xs.reduce((s, c) => s + c.close, 0) / Math.max(1, xs.length), actual: xs.filter((c) => c.passed).length / Math.max(1, xs.length) };
      });
      const v = m.getVault();
      const agents = WORKERS.map((w) => { const r = m.getAgent(w.id); return { id: w.id, kind: w.kind, jobs: r.jobs, passes: r.passes, fails: r.fails, prior: m.agentPriorBps(w.id) / 10000, earned: f(r.earned), slashed: f(r.slashed), brier: r.jobs ? r.brierBpsSum / r.jobs / 10000 : null, hiredAt: workerState.get(w.id).hiredAt, joins: w.joins }; });
      const traders = TRADERS.map((t) => ({ id: t.id, kind: t.kind, pnl: f(m.wallets.get(t.id) || 0n) }));
      return {
        ticks: tick, jobsPosted: stats.posted, jobsResolved: stats.closes.length, jobsCancelled: stats.cancelled,
        passRate: stats.closes.filter((c) => c.passed).length / Math.max(1, stats.closes.length),
        brierOpeningPrior: brier('prior'), brierMarketClose: brier('close'), calibration: bins,
        vault: { start: vaultStart, assets: f(v.totalAssets), pnl: f(v.realizedPnl), fees: f(v.feesEarned), returnPct: (f(v.totalAssets) / vaultStart - 1) * 100 },
        buyers: { consequentialLoss: stats.consequentialLoss, coverPaid: stats.coverPaid, premiums: stats.premiums, uninsuredLoss: stats.uninsuredLoss, insuredLossAfterCover: stats.insuredLossAfterCover },
        sleeper: { turnedOnJob: stats.sleeperTurned, closes: stats.closes.filter((c) => c.agent === 'sleeper.agent' && !c.passed).map((c) => ({ job: c.job, prior: c.prior, close: c.close })) },
        rookies: stats.rookieHires, rejections: stats.rejections.slice(0, 10), agents, traders,
      };
    }

    return { market: m, step, report, stats, live, get tick() { return tick; }, WORKERS, TRADERS, BUYERS };
  }

  // ------------------------------------------------------------------
  // Story mode: the scripted demo used in the video. Deterministic.
  // ------------------------------------------------------------------
  function createStory() {
    const m = new Actuary();
    const pct = (id) => (m.passPriceBps(id) / 100).toFixed(1) + '%';
    const ctx = { m, nova: null, sleeper: null, novaCover: 0n, globexCover: 0n, log: [] };
    const at = (h) => { m.now = BigInt(h) * HOUR; };
    const steps = [
      { title: 'Underwriters fund the vault', run() {
        m.vaultDeposit('lp-alpha.near', n(300)); m.vaultDeposit('lp-beta.near', n(200));
        return 'Two liquidity providers deposit 500 NEAR into the underwriting vault. Every new job market is seeded from it, and it earns the trading fees.'; } },
      { title: 'A track record, written on-chain', run() {
        for (let i = 0; i < 12; i++) { at(i); const id = m.postJob('history.buyer', { agent: 'sleeper.agent', verifier: VERIFIER, spec: 'past', deadline: m.now + HOUR }, n(1)); m.acceptJob('sleeper.agent', id, n(0.1)); m.resolve(VERIFIER, id, 'Pass', 'tests passed'); }
        return `sleeper.agent has delivered 12 jobs in a row. Its record lives in the contract, so its next market opens at ${(m.agentPriorBps('sleeper.agent') / 100).toFixed(1)}%. nova.agent is brand new: it opens at ${(m.agentPriorBps('nova.agent') / 100).toFixed(1)}%.`; } },
      { title: 'A rookie gets a real job', run() {
        at(20); ctx.nova = m.postJob('acme.buyer', { agent: 'nova.agent', verifier: VERIFIER, spec: 'sha256:slugify-spec+24-tests', deadline: m.now + 24n * HOUR }, n(10));
        return `acme.buyer hires nova.agent to write a function that must pass a 24-test suite, and escrows 10 NEAR. With no history, nova's market would open at ${pct(ctx.nova)}. On most agent marketplaces, nova would never win this job.`; } },
      { title: 'Skin in the game', run() {
        at(21); const q = m.acceptJob('nova.agent', ctx.nova, n(4));
        return `nova accepts by staking a 4 NEAR bond on PASS. The vault seeds the market and the bond moves the price to ${(q.passPriceAfterBps / 100).toFixed(1)}%. If nova fails, the bond goes to whoever insured against it.`; } },
      { title: 'The buyer buys cover', run() {
        at(21); const cost = m.costFor(ctx.nova, 'Fail', n(6)); m.buy('acme.buyer', ctx.nova, 'Fail', cost, 0n, 'cover'); ctx.novaCover = m.getPosition(ctx.nova, 'acme.buyer').fail;
        return `If nova misses, acme loses its launch slot, about 6 NEAR. Its payment is already safe in escrow, so acme buys 6 NEAR of cover (FAIL shares) for ${f(cost).toFixed(2)} NEAR. The market set that premium. PASS is now ${pct(ctx.nova)}.`; } },
      { title: 'The swarm prices the risk', run() {
        at(22); tradeToward(m, 'actuary-1.swarm', ctx.nova, 0.64, 3, 0.25, 'actuary'); tradeToward(m, 'actuary-2.swarm', ctx.nova, 0.66, 3, 0.25, 'actuary');
        return `Underwriter agents read nova's public profile and the spec, and bid PASS up to ${pct(ctx.nova)}. Nobody had to trust nova. They only had to be willing to price it.`; } },
      { title: 'Heartbeats come in on time', run() {
        const ps = [];
        for (const h of [27, 33, 39]) { at(h); ps.push((m.heartbeat(VERIFIER, ctx.nova, true, 'sha256:commit') / 100).toFixed(1) + '%'); }
        return `nova hits three milestones on schedule. The verifier attests each one, and the vault's market maker updates its own quote: ${ps.join(' → ')}. That is a live, money-backed answer to "will this get done?"`; } },
      { title: 'A TEE verifier runs the tests', run() {
        at(43); m.resolve(VERIFIER, ctx.nova, 'Pass', 'sha256:9f2c… tests 24/24 passed (attested)');
        const bondBack = m.redeem('nova.agent', ctx.nova);
        for (const who of ['actuary-1.swarm', 'actuary-2.swarm']) { try { m.redeem(who, ctx.nova); } catch (e) { /* no PASS shares */ } }
        const r = m.getAgent('nova.agent');
        return `The verifier runs the 24 tests on the delivered code inside a TEE and all of them pass. nova is paid 9.80 NEAR from escrow (2% goes to the vault) and its 4 NEAR bond comes back as ${f(bondBack).toFixed(2)} NEAR. Its record is now ${r.passes} pass / ${r.fails} fail. acme paid a small premium and got its work.`; } },
      { title: 'The trusted agent takes a big job', run() {
        at(50); ctx.sleeper = m.postJob('globex.buyer', { agent: 'sleeper.agent', verifier: VERIFIER, spec: 'sha256:migration-spec', deadline: m.now + 24n * HOUR, grace: 2n * HOUR }, n(20));
        m.acceptJob('sleeper.agent', ctx.sleeper, n(2));
        const cost = m.costFor(ctx.sleeper, 'Fail', n(15)); m.buy('globex.buyer', ctx.sleeper, 'Fail', cost, 0n, 'cover'); ctx.globexCover = m.getPosition(ctx.sleeper, 'globex.buyer').fail;
        return `globex.buyer hires sleeper.agent for a 20 NEAR database migration. Its record makes cover cheap: 15 NEAR of protection costs globex ${f(cost).toFixed(2)} NEAR. PASS opens at ${pct(ctx.sleeper)}.`; } },
      { title: 'The agent tries to bet against itself', run() {
        at(51); try { m.buy('sleeper.agent', ctx.sleeper, 'Fail', n(15)); return 'unexpected'; } catch (e) {
          return `sleeper.agent plans to walk away with the reputation it farmed. First it tries to buy FAIL on its own job. The contract refuses: "${e.message}".`; } } },
      { title: 'Heartbeats go missing', run() {
        const ps = [];
        for (const h of [56, 62, 68]) { at(h); ps.push((m.heartbeat(VERIFIER, ctx.sleeper, false, 'missed') / 100).toFixed(1) + '%'); }
        return `Three milestones are missed. The verifier attests each miss and the price falls on its own: ${ps.join(' → ')}, long before the deadline. Twelve straight passes did not hide the risk, because the market watches the work, not the résumé.`; } },
      { title: 'Deadline passes. Anyone can close it.', run() {
        at(77); m.expire(ctx.sleeper);
        const paid = m.redeem('globex.buyer', ctx.sleeper);
        return `No delivery after the grace period, so anyone can call expire() and the job resolves FAIL. globex gets its 20 NEAR back from escrow and its cover pays ${f(paid).toFixed(2)} NEAR for the damage. sleeper.agent's 2 NEAR bond is gone. Escrow gets your money back. Actuary makes you whole.`; } },
      { title: 'Reputation updates itself', run() {
        const r = m.getAgent('sleeper.agent'); const v = m.getVault();
        return `sleeper.agent's record is now ${r.passes} / ${r.fails} and its next market opens at ${(m.agentPriorBps('sleeper.agent') / 100).toFixed(1)}%. nova's next one opens at ${(m.agentPriorBps('nova.agent') / 100).toFixed(1)}%. The vault paid its share of globex's claim and holds ${f(v.totalAssets).toFixed(2)} NEAR of its 500 (in the 320-job swarm run it ends up ahead). Insurance, a bond and a credit score, all from one market.`; } },
    ];
    return { m, ctx, steps, f };
  }

  return { createSwarm, createStory, tradeToward, rng, n, f, WORKERS, TRADERS, BUYERS, VERIFIER };
});
