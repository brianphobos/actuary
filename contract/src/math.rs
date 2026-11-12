//! Integer market math shared by the contract and mirrored in `sim/engine.js`.
//! Test vectors in `tests.rs` and `sim/test.js` check that both agree exactly.

uint::construct_uint! {
    pub struct U256(4);
}

pub const BPS_U: u128 = 10_000;

pub fn mul_div(a: u128, b: u128, c: u128) -> u128 {
    assert!(c > 0, "division by zero");
    (U256::from(a) * U256::from(b) / U256::from(c)).as_u128()
}

pub fn mul_div_ceil(a: u128, b: u128, c: u128) -> u128 {
    assert!(c > 0, "division by zero");
    let n = U256::from(a) * U256::from(b);
    let c = U256::from(c);
    let q = n / c;
    if q * c == n { q.as_u128() } else { (q + 1).as_u128() }
}

/// Opening PASS probability from an agent's record: Laplace-smoothed pass rate,
/// clamped to [5%, 95%] so no market opens at certainty. A new agent opens at 50%.
pub fn prior_from_record(passes: u32, fails: u32) -> u32 {
    let p = (passes as u64 + 1) * 10_000 / (passes as u64 + fails as u64 + 2);
    p.clamp(500, 9_500) as u32
}

/// PASS price in bps for an FPMM with these reserves.
pub fn price_pass_bps(pool_pass: u128, pool_fail: u128) -> u32 {
    let total = pool_pass + pool_fail;
    if total == 0 {
        return 5_000;
    }
    mul_div(pool_fail, BPS_U, total) as u32
}

pub struct Seed {
    pub pool_pass: u128,
    pub pool_fail: u128,
    pub extra_pass: u128,
    pub extra_fail: u128,
}

/// Seed a pool with `liquidity` collateral at PASS probability `p_bps`.
/// The collateral mints `liquidity` of each outcome; the scarcer side is kept
/// in full, the other is trimmed so the price matches, and the trimmed tokens
/// stay with the vault.
pub fn seed_pool(liquidity: u128, p_bps: u32) -> Seed {
    let p = p_bps as u128;
    if p >= 5_000 {
        // PASS is likely: PASS reserve is smaller. price_pass = fail/(pass+fail) = p
        let pool_pass = mul_div(liquidity, BPS_U - p, p);
        Seed { pool_pass, pool_fail: liquidity, extra_pass: liquidity - pool_pass, extra_fail: 0 }
    } else {
        let pool_fail = mul_div(liquidity, p, BPS_U - p);
        Seed { pool_pass: liquidity, pool_fail, extra_pass: 0, extra_fail: liquidity - pool_fail }
    }
}

pub struct BuyResult {
    pub shares: u128,
    pub pool_pass: u128,
    pub pool_fail: u128,
}

/// Fixed Product Market Maker buy. `invest` collateral mints `invest` of both
/// outcomes into the pool; the buyer takes out enough of the chosen outcome to
/// restore the product of reserves. Rounds in the pool's favour.
pub fn fpmm_buy(pool_pass: u128, pool_fail: u128, buy_pass: bool, invest: u128) -> BuyResult {
    assert!(pool_pass > 0 && pool_fail > 0, "empty pool");
    let (mine, other) = if buy_pass { (pool_pass, pool_fail) } else { (pool_fail, pool_pass) };
    let other_new = other + invest;
    let mine_new = mul_div_ceil(mine, other, other_new);
    let shares = mine + invest - mine_new;
    if buy_pass {
        BuyResult { shares, pool_pass: mine_new, pool_fail: other_new }
    } else {
        BuyResult { shares, pool_pass: other_new, pool_fail: mine_new }
    }
}

/// Bayes update of a PASS price on an attested heartbeat.
/// `a_bps` = P(heartbeat on time | job will pass), `b_bps` = P(on time | job will fail).
/// Clamped to [3%, 97%] so a heartbeat alone never makes the market certain.
pub fn bayes_bps(p_bps: u32, on_time: bool, a_bps: u32, b_bps: u32) -> u32 {
    let p = p_bps as u128;
    let (a, b) = if on_time { (a_bps as u128, b_bps as u128) } else { (BPS_U - a_bps as u128, BPS_U - b_bps as u128) };
    let num = p * a;
    let den = p * a + (BPS_U - p) * b;
    ((num * BPS_U / den) as u32).clamp(300, 9_700)
}

/// Re-centre a pool at `p_bps` using only outcome tokens the vault owns
/// (pool reserves plus the extra tokens it holds outside the pool).
pub fn reprice_pool(pool_pass: u128, pool_fail: u128, extra_pass: u128, extra_fail: u128, p_bps: u32) -> Seed {
    let p = p_bps as u128;
    let vp = pool_pass + extra_pass;
    let vf = pool_fail + extra_fail;
    let mut pf = vf;
    let mut pp = mul_div(pf, BPS_U - p, p);
    if pp > vp {
        pp = vp;
        pf = mul_div(pp, p, BPS_U - p);
    }
    Seed { pool_pass: pp, pool_fail: pf, extra_pass: vp - pp, extra_fail: vf - pf }
}
