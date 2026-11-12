/*
 * Actuary engine: a line-for-line JavaScript mirror of the NEAR contract
 * (contract/src/lib.rs and math.rs), using BigInt so every number matches the
 * contract exactly. It runs in Node (require) and in the browser (window.ActuaryEngine).
 */
(function (root, factory) {
  const api = factory();
  if (typeof module === 'object' && module.exports) module.exports = api;
  else root.ActuaryEngine = api;
})(typeof self !== 'undefined' ? self : this, function () {
  'use strict';
  const BPS = 10000n;
  const NEAR = 10n ** 24n;
  const MIN_TRADE = NEAR / 100n;
  const HOUR = 3600n * 10n ** 9n;
  const DEFAULT_GRACE = HOUR;

  // ---------------- math.rs ----------------
  const mulDiv = (a, b, c) => { if (c === 0n) throw new Error('division by zero'); return (a * b) / c; };
  const mulDivCeil = (a, b, c) => { const n = a * b; const q = n / c; return q * c === n ? q : q + 1n; };
  function priorFromRecord(passes, fails) {
    const p = Number(((BigInt(passes) + 1n) * 10000n) / (BigInt(passes) + BigInt(fails) + 2n));
    return Math.min(9500, Math.max(500, p));
  }
  function pricePassBps(pp, pf) {
    const t = pp + pf;
    if (t === 0n) return 5000;
    return Number(mulDiv(pf, BPS, t));
  }
  function seedPool(liq, pBps) {
    const p = BigInt(pBps);
    if (p >= 5000n) {
      const poolPass = mulDiv(liq, BPS - p, p);
      return { poolPass, poolFail: liq, extraPass: liq - poolPass, extraFail: 0n };
    }
    const poolFail = mulDiv(liq, p, BPS - p);
    return { poolPass: liq, poolFail, extraPass: 0n, extraFail: liq - poolFail };
  }
  function fpmmBuy(pp, pf, buyPass, invest) {
    if (pp <= 0n || pf <= 0n) throw new Error('empty pool');
    const mine = buyPass ? pp : pf, other = buyPass ? pf : pp;
    const otherNew = other + invest;
    const mineNew = mulDivCeil(mine, other, otherNew);
    const shares = mine + invest - mineNew;
    return buyPass ? { shares, poolPass: mineNew, poolFail: otherNew } : { shares, poolPass: otherNew, poolFail: mineNew };
  }

  /** Bayes update of a PASS price on an attested heartbeat. a = P(on time | will pass), b = P(on time | will fail), in bps. */
  function bayesBps(pBps, onTime, aBps, bBps) {
    const p = BigInt(pBps), a = BigInt(onTime ? aBps : 10000 - aBps), b = BigInt(onTime ? bBps : 10000 - bBps);
    const num = p * a, den = p * a + (10000n - p) * b;
    const r = Number((num * 10000n) / den);
    return Math.min(9700, Math.max(300, r));
  }
  /** Re-centre the pool at `pBps` using only tokens the vault owns (pool + extra). */
  function repricePool(job, pBps) {
    const p = BigInt(pBps);
    const vp = job.poolPass + job.extraPass, vf = job.poolFail + job.extraFail;
    // price = fail / (pass + fail)  =>  pass = fail * (1 - p) / p
    let pf = vf, pp = mulDiv(pf, BPS - p, p);
    if (pp > vp) { pp = vp; pf = mulDiv(pp, p, BPS - p); }
    return { poolPass: pp, poolFail: pf, extraPass: vp - pp, extraFail: vf - pf };
  }

  class ContractError extends Error {}
  const require_ = (cond, msg) => { if (!cond) throw new ContractError(msg); };

  // ---------------- lib.rs ----------------
  class Actuary {
    constructor({ feeBps = 300, liquidityBps = 10000, minBondBps = 1000, takeBps = 200, hbPassBps = 9200, hbFailBps = 3000 } = {}) {
      Object.assign(this, { feeBps, liquidityBps, minBondBps, takeBps, hbPassBps, hbFailBps });
      this.jobs = []; this.positions = new Map(); this.agents = new Map();
      this.vaultShares = new Map(); this.vaultTotalShares = 0n;
      this.vaultFree = 0n; this.vaultLocked = 0n; this.vaultFees = 0n; this.vaultPnl = 0n;
      this.events = []; this.wallets = new Map(); this.now = 0n;
    }
    // bookkeeping helpers (not in the contract): net flow per account, event log
    _flow(who, amt) { this.wallets.set(who, (this.wallets.get(who) || 0n) + amt); }
    _emit(e) { e.t = this.now; this.events.push(e); }
    vaultTotalAssets() { return this.vaultFree + this.vaultLocked; }

    vaultDeposit(who, amount) {
      require_(amount >= MIN_TRADE, 'deposit too small');
      const assets = this.vaultTotalAssets();
      const minted = this.vaultTotalShares === 0n || assets === 0n ? amount : mulDiv(amount, this.vaultTotalShares, assets);
      this.vaultShares.set(who, (this.vaultShares.get(who) || 0n) + minted);
      this.vaultTotalShares += minted; this.vaultFree += amount; this._flow(who, -amount);
      this._emit({ type: 'vault_deposit', who, amount });
      return minted;
    }
    vaultWithdraw(who, shares) {
      const cur = this.vaultShares.get(who) || 0n;
      require_(shares > 0n && shares <= cur, 'not enough vault shares');
      const amount = mulDiv(shares, this.vaultTotalAssets(), this.vaultTotalShares);
      require_(amount <= this.vaultFree, 'capital is locked in open jobs; try later');
      this.vaultShares.set(who, cur - shares); this.vaultTotalShares -= shares; this.vaultFree -= amount;
      this._flow(who, amount); return amount;
    }
    postJob(buyer, { agent, verifier, spec, deadline, grace }, payment) {
      require_(payment >= MIN_TRADE, 'payment too small');
      require_(agent !== buyer, 'buyer cannot hire itself');
      require_(verifier !== agent, 'agent cannot verify its own work');
      require_(deadline > this.now, 'deadline is in the past');
      const rec = this.getAgent(agent);
      const id = this.jobs.length;
      this.jobs.push({ id, buyer, agent, verifier, spec, payment, deadline, grace: grace ?? DEFAULT_GRACE,
        status: 'Posted', priorBps: priorFromRecord(rec.passes, rec.fails), bond: 0n, liquidity: 0n,
        poolPass: 0n, poolFail: 0n, extraPass: 0n, extraFail: 0n, fees: 0n, closingBps: 0, evidence: null,
        created: this.now, history: [] });
      this._flow(buyer, -payment);
      this._emit({ type: 'job_posted', job: id, buyer, agent, payment, priorBps: this.jobs[id].priorBps });
      return id;
    }
    acceptJob(who, id, bond) {
      const job = this.job(id);
      require_(who === job.agent, 'only the hired agent can accept');
      require_(job.status === 'Posted', 'job is not waiting for acceptance');
      require_(this.now < job.deadline, 'deadline passed');
      let minBond = (job.payment * BigInt(this.minBondBps)) / BPS; if (minBond < MIN_TRADE) minBond = MIN_TRADE;
      require_(bond >= minBond, `bond must be at least ${minBond} yoctoNEAR`);
      const liq = (job.payment * BigInt(this.liquidityBps)) / BPS;
      require_(liq <= this.vaultFree, 'underwriting vault has too little free capital');
      this.vaultFree -= liq; this.vaultLocked += liq;
      const s = seedPool(liq, job.priorBps);
      Object.assign(job, { liquidity: liq, poolPass: s.poolPass, poolFail: s.poolFail, extraPass: s.extraPass, extraFail: s.extraFail, status: 'Active', bond });
      job.history.push({ t: this.now, p: job.priorBps, kind: 'open' });
      const q = this._executeBuy(job, who, 'Pass', bond, 0n, 'bond');
      const rec = this.getAgent(job.agent); rec.bonded += bond; this.agents.set(job.agent, rec);
      this._emit({ type: 'job_accepted', job: id, agent: who, bond, passBps: q.passPriceAfterBps });
      return q;
    }
    buy(who, id, outcome, amount, minShares = 0n, tag) {
      require_(amount >= MIN_TRADE, 'trade too small');
      const job = this.job(id);
      require_(job.status === 'Active', 'market is not open');
      require_(this.now < job.deadline, 'market closed at the deadline');
      require_(who !== job.verifier, 'the verifier cannot trade on jobs it judges');
      require_(!(who === job.agent && outcome === 'Fail'), 'an agent cannot bet against its own work');
      return this._executeBuy(job, who, outcome, amount, minShares, tag);
    }
    /**
     * The verifier attests a progress heartbeat (on time or missed). The vault's
     * market maker reprices itself with a Bayes update, so public progress news
     * moves the quote directly instead of being a free option for the fastest trader.
     */
    heartbeat(who, id, onTime, evidence) {
      const job = this.job(id);
      require_(who === job.verifier, 'only the verifier can attest heartbeats');
      require_(job.status === 'Active', 'job is not active');
      require_(this.now < job.deadline, 'market closed at the deadline');
      const before = pricePassBps(job.poolPass, job.poolFail);
      const target = bayesBps(before, onTime, this.hbPassBps, this.hbFailBps);
      Object.assign(job, repricePool(job, target));
      const after = pricePassBps(job.poolPass, job.poolFail);
      job.heartbeats = (job.heartbeats || 0) + 1; job.missed = (job.missed || 0) + (onTime ? 0 : 1);
      job.history.push({ t: this.now, p: after, kind: onTime ? 'hb_ok' : 'hb_miss' });
      this._emit({ type: 'heartbeat', job: id, onTime, before, after, evidence });
      return after;
    }
    resolve(who, id, outcome, evidence) {
      const job = this.job(id);
      require_(who === job.verifier, 'only the verifier can resolve');
      require_(job.status === 'Active', 'job is not active');
      this._settle(job, outcome, evidence);
    }
    expire(id) {
      const job = this.job(id);
      require_(this.now >= job.deadline + job.grace, 'too early to expire');
      if (job.status === 'Active') this._settle(job, 'Fail', 'expired: no verified delivery');
      else if (job.status === 'Posted') { job.status = 'Cancelled'; this._flow(job.buyer, job.payment); this._emit({ type: 'job_cancelled', job: id }); }
      else throw new ContractError('job already closed');
    }
    redeem(who, id) {
      const job = this.job(id);
      const key = `${id}:${who}`; const pos = this.positions.get(key) || { pass: 0n, fail: 0n };
      let payout;
      if (job.status === 'Passed') payout = pos.pass; else if (job.status === 'Failed') payout = pos.fail; else throw new ContractError('job not resolved');
      require_(payout > 0n, 'nothing to redeem');
      this.positions.delete(key); this._flow(who, payout);
      this._emit({ type: 'redeem', job: id, who, payout });
      return payout;
    }
    // views
    job(id) { const j = this.jobs[id]; require_(j, 'no such job'); return j; }
    passPriceBps(id) { const j = this.job(id); return j.status === 'Posted' ? j.priorBps : pricePassBps(j.poolPass, j.poolFail); }
    quote(id, outcome, amount) {
      const j = this.job(id); require_(j.status === 'Active', 'market is not open');
      const fee = (amount * BigInt(this.feeBps)) / BPS;
      const r = fpmmBuy(j.poolPass, j.poolFail, outcome === 'Pass', amount - fee);
      return { shares: r.shares, fee, avgPriceBps: Number(mulDiv(amount, BPS, r.shares > 0n ? r.shares : 1n)), passPriceAfterBps: pricePassBps(r.poolPass, r.poolFail) };
    }
    /** Smallest spend that buys at least `shares` of `outcome` (binary search over quote). */
    costFor(id, outcome, shares) {
      let lo = MIN_TRADE, hi = shares * 3n + MIN_TRADE;
      if (this.quote(id, outcome, lo).shares >= shares) return lo;
      for (let i = 0; i < 90 && hi - lo > NEAR / 10000n; i++) {
        const mid = (lo + hi) / 2n;
        if (this.quote(id, outcome, mid).shares >= shares) hi = mid; else lo = mid;
      }
      return hi;
    }
    getPosition(id, who) { return this.positions.get(`${id}:${who}`) || { pass: 0n, fail: 0n }; }
    getAgent(a) {
      return this.agents.get(a) || { jobs: 0, passes: 0, fails: 0, sumPriorBps: 0, brierBpsSum: 0, bonded: 0n, slashed: 0n, earned: 0n };
    }
    agentPriorBps(a) { const r = this.getAgent(a); return priorFromRecord(r.passes, r.fails); }
    getVault() { return { totalAssets: this.vaultTotalAssets(), free: this.vaultFree, locked: this.vaultLocked, totalShares: this.vaultTotalShares, realizedPnl: this.vaultPnl, feesEarned: this.vaultFees }; }

    _executeBuy(job, who, outcome, amount, minShares, tag) {
      const fee = (amount * BigInt(this.feeBps)) / BPS;
      const r = fpmmBuy(job.poolPass, job.poolFail, outcome === 'Pass', amount - fee);
      require_(r.shares >= minShares, 'slippage: fewer shares than min_shares');
      job.poolPass = r.poolPass; job.poolFail = r.poolFail; job.fees += fee;
      const key = `${job.id}:${who}`; const pos = this.positions.get(key) || { pass: 0n, fail: 0n };
      if (outcome === 'Pass') pos.pass += r.shares; else pos.fail += r.shares;
      this.positions.set(key, pos); this._flow(who, -amount);
      const after = pricePassBps(r.poolPass, r.poolFail);
      job.history.push({ t: this.now, p: after, kind: tag || outcome, who });
      this._emit({ type: 'trade', job: job.id, who, outcome, amount, shares: r.shares, passBps: after, tag });
      return { shares: r.shares, fee, avgPriceBps: Number(mulDiv(amount, BPS, r.shares > 0n ? r.shares : 1n)), passPriceAfterBps: after };
    }
    _settle(job, outcome, evidence) {
      const closing = pricePassBps(job.poolPass, job.poolFail);
      job.closingBps = closing; job.evidence = evidence; job.status = outcome === 'Pass' ? 'Passed' : 'Failed';
      // Escrow: on PASS the agent is paid minus the underwriting take; on FAIL the buyer is refunded in full.
      const take = outcome === 'Pass' ? (job.payment * BigInt(this.takeBps)) / BPS : 0n;
      this._flow(outcome === 'Pass' ? job.agent : job.buyer, job.payment - take);
      const back = (outcome === 'Pass' ? job.poolPass + job.extraPass : job.poolFail + job.extraFail) + job.fees + take;
      this.vaultLocked -= job.liquidity; this.vaultFree += back; this.vaultFees += job.fees + take; this.vaultPnl += back - job.liquidity;
      const rec = this.getAgent(job.agent);
      rec.jobs += 1; rec.sumPriorBps += job.priorBps;
      const err = closing - (outcome === 'Pass' ? 10000 : 0); rec.brierBpsSum += Math.trunc((err * err) / 10000);
      if (outcome === 'Pass') { rec.passes += 1; rec.earned += job.payment - take; } else { rec.fails += 1; rec.slashed += job.bond; }
      this.agents.set(job.agent, rec);
      job.history.push({ t: this.now, p: outcome === 'Pass' ? 10000 : 0, kind: 'resolve' });
      this._emit({ type: 'job_resolved', job: job.id, outcome, closingBps: closing, evidence, vaultBack: back - job.liquidity });
    }
  }

  return { Actuary, ContractError, bayesBps, repricePool, BPS, NEAR, MIN_TRADE, HOUR, mulDiv, mulDivCeil, priorFromRecord, pricePassBps, seedPool, fpmmBuy };
});
