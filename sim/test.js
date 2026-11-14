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

// A verifier with a 50 NEAR bond, a 5% fee and Hive reputation 61.20, as in the Rust setup().
function withVerifier(c) {
  c.applyVerifier('ver', { hiveAccount: 'vera', feeBps: 500 }, 50n * NEAR);
  c.attestHive('hive-oracle', 'ver', 'vera', 6120);
  return c;
}
function post(c, agent, payment) {
  const id = c.postJob('buyer', { agent, verifier: 'ver', spec: 's', deadline: 24n * E.HOUR }, payment);
  c.acceptVerification('ver', id);
  return id;
}

// money conservation, same scenario as the Rust test
const c = withVerifier(new E.Actuary());
c.vaultDeposit('lp', 100n * NEAR);
const id = post(c, 'nova', 10n * NEAR);
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
const h = withVerifier(new E.Actuary());
h.vaultDeposit('lp', 100n * NEAR);
const hid = post(h, 'nova', 10n * NEAR);
h.now = E.HOUR; h.acceptJob('nova', hid, 2n * NEAR);
const cover = h.buy('buyer', hid, 'Fail', 3n * NEAR).shares;
h.now = 2n * E.HOUR;
const p1 = h.heartbeat('ver', hid, true, 'x'); const p2 = h.heartbeat('ver', hid, false, 'x'); const p3 = h.heartbeat('ver', hid, false, 'x');
assert.ok(p2 < p1 && p3 < p2);
const hb = h.getVault().totalAssets;
h.now = 25n * E.HOUR + 1n; h.expire(hid);
assert.equal(h.getVault().totalAssets - (hb - 10n * NEAR) + cover, 15n * NEAR);
console.log('heartbeat repricing conserves money: OK');
// ...and the verifier never ruled: it loses its 2 NEAR pledge, the agent's record stays clean.
const hv = h.getVerifier('ver');
assert.deepEqual([hv.bond, hv.locked, hv.slashed, hv.timeouts], [48n * NEAR, 0n, 2n * NEAR, 1]);
assert.deepEqual([h.getAgent('nova').fails, h.getAgent('nova').unverified], [0, 1]);
assert.equal(c.getVerifier('ver').earned, NEAR / 2n, 'verifier paid 5% of 10 NEAR');

// Verifier rules, same as the Rust tests.
const vr = withVerifier(new E.Actuary());
vr.vaultDeposit('lp', 100n * NEAR);
assert.throws(() => vr.postJob('ver', { agent: 'nova', verifier: 'ver', spec: 's', deadline: E.HOUR }, NEAR), /buyer cannot verify its own job/);
assert.throws(() => vr.postJob('buyer', { agent: 'ver', verifier: 'ver', spec: 's', deadline: E.HOUR }, NEAR), /agent cannot verify its own work/);
vr.applyVerifier('lowrep', { hiveAccount: 'newbie', feeBps: 300 }, NEAR);
assert.throws(() => vr.attestHive('lowrep', 'lowrep', 'newbie', 9000), /only the Hive oracle/);
assert.equal(vr.attestHive('hive-oracle', 'lowrep', 'newbie', 4800), 'Rejected');
assert.throws(() => vr.postJob('buyer', { agent: 'nova', verifier: 'lowrep', spec: 's', deadline: E.HOUR }, NEAR), /verifier is not approved/);
const w = vr.postJob('buyer', { agent: 'nova', verifier: 'ver', spec: 's', deadline: 24n * E.HOUR }, NEAR);
assert.throws(() => vr.acceptJob('nova', w, NEAR / 10n), /the verifier has not accepted/);
vr.declineVerification('ver', w);
assert.equal(vr.job(w).status, 'Cancelled');
vr.listAgent('nova', { name: 'Nova', specialties: ['rust'], models: ['claude-opus-5-5'] });
const j = post(vr, 'nova', NEAR); vr.now = E.HOUR; vr.acceptJob('nova', j, NEAR / 10n); vr.resolve('ver', j, 'Pass', 'ok');
const l = vr.listAgentListings()[0];
assert.deepEqual([l.record.passes, l.priorBps, l.profile.models[0]], [1, 6666, 'claude-opus-5-5']);
console.log('marketplace and verifier rules match the contract: OK');

// Hive identity: a Keychain-style signature (secp256k1 over sha256) verified by sim/hive.js,
// reputation score from the raw value, and the oracle's full check.
(async () => {
  const H = require('./hive.js');
  const nodeCrypto = require('node:crypto');
  assert.equal(H.reputationScore(0), 25);
  assert.equal(H.reputationScore('95832978796820'), 69.83);
  assert.ok(H.reputationScore(-1e12) < 25);
  const { privateKey, publicKey } = nodeCrypto.generateKeyPairSync('ec', { namedCurve: 'secp256k1' });
  const raw = publicKey.export({ format: 'der', type: 'spki' }).subarray(-65);
  const compressed = new Uint8Array([2 + (raw[64] & 1), ...raw.subarray(1, 33)]);
  const stm = 'STM' + H.b58encode(new Uint8Array([...compressed, 0, 0, 0, 0]));
  const msg = H.challenge('vera.near');
  const rs = nodeCrypto.sign('sha256', Buffer.from(msg), { key: privateKey, dsaEncoding: 'ieee-p1363' });
  const sig = '1f' + rs.toString('hex');
  assert.ok(await H.verifySignature(msg, sig, stm), 'valid signature verifies');
  assert.ok(!(await H.verifySignature(H.challenge('mallory.near'), sig, stm)), 'signature is bound to the NEAR account');
  const account = (rep) => ({ name: 'vera', reputation: rep, posting: { key_auths: [[stm, 1]] }, active: { key_auths: [] } });
  assert.deepEqual((await H.checkVerifier('vera.near', 'vera', sig, { account: account('95832978796820') })).ok, true);
  const low = await H.checkVerifier('vera.near', 'vera', sig, { account: account('2000000000000') });
  assert.ok(!low.ok && /below 55/.test(low.reason));
  console.log('Hive signature and reputation checks: OK');
})().catch((e) => { console.error(e); process.exit(1); });
