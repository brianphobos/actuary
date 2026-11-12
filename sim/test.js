// Checks that the JS engine matches the Rust contract exactly (same vectors as contract/src/tests.rs).
const assert = require('node:assert/strict');
const E = require('./engine.js');
const { NEAR } = E;
let s = E.seedPool(10n * NEAR, 8000);
assert.equal(s.poolPass, 2_500_000_000_000_000_000_000_000n);
assert.equal(s.extraPass, 7_500_000_000_000_000_000_000_000n);
assert.equal(E.pricePassBps(s.poolPass, s.poolFail), 8000);
let r = E.fpmmBuy(10n * NEAR, 10n * NEAR, true, NEAR);
assert.equal(r.shares, 1_909_090_909_090_909_090_909_090n);
assert.equal(E.pricePassBps(r.poolPass, r.poolFail), 5475);
r = E.fpmmBuy(10n * NEAR, 10n * NEAR, false, 3n * NEAR);
assert.equal(r.shares, 5_307_692_307_692_307_692_307_692n);
assert.equal(E.priorFromRecord(0, 0), 5000);
assert.equal(E.priorFromRecord(9, 0), 9090);
assert.equal(E.priorFromRecord(0, 30), 500);
assert.equal(E.bayesBps(5000, true, 9200, 3000), 7540);
assert.equal(E.bayesBps(9000, false, 9200, 3000), 5070);
s = E.seedPool(10n * NEAR, 5000);
let rp = E.repricePool(s, 7540);
assert.equal(rp.poolPass, 3_262_599_469_496_021_220_159_151n);
assert.equal(rp.extraPass, 6_737_400_530_503_978_779_840_849n);
s = E.seedPool(10n * NEAR, 9000);
rp = E.repricePool(s, 2000);
assert.equal(rp.poolFail, 2_500_000_000_000_000_000_000_000n);
assert.equal(rp.extraFail, 7_500_000_000_000_000_000_000_000n);

// money conservation, same scenario as the Rust test
const c = new E.Actuary();
c.vaultDeposit('lp', 100n * NEAR);
const id = c.postJob('buyer', { agent: 'nova', verifier: 'ver', spec: 's', deadline: 24n * E.HOUR }, 10n * NEAR);
c.now = E.HOUR;
c.acceptJob('nova', id, 3n * NEAR);
c.buy('buyer', id, 'Fail', 2n * NEAR);
const b = c.buy('spec', id, 'Pass', 4n * NEAR).shares;
const before = c.getVault().totalAssets;
c.resolve('ver', id, 'Pass', 'ok');
const back = c.getVault().totalAssets - (before - 10n * NEAR);
assert.equal(back + c.getPosition(id, 'nova').pass + b, 19n * NEAR + NEAR / 5n);
assert.throws(() => c.buy('nova', id, 'Fail', NEAR), /market is not open/);
console.log('engine.js matches contract vectors: OK (19 checks)');

// heartbeat scenario, same as the Rust test
const h = new E.Actuary();
h.vaultDeposit('lp', 100n * NEAR);
const hid = h.postJob('buyer', { agent: 'nova', verifier: 'ver', spec: 's', deadline: 24n * E.HOUR }, 10n * NEAR);
h.now = E.HOUR; h.acceptJob('nova', hid, 2n * NEAR);
const cover = h.buy('buyer', hid, 'Fail', 3n * NEAR).shares;
h.now = 2n * E.HOUR;
const p1 = h.heartbeat('ver', hid, true, 'x'); const p2 = h.heartbeat('ver', hid, false, 'x'); const p3 = h.heartbeat('ver', hid, false, 'x');
assert.ok(p2 < p1 && p3 < p2);
const hb = h.getVault().totalAssets;
h.now = 25n * E.HOUR + 1n; h.expire(hid);
assert.equal(h.getVault().totalAssets - (hb - 10n * NEAR) + cover, 15n * NEAR);
console.log('heartbeat repricing conserves money: OK');
