use super::*;
use near_sdk::test_utils::{get_logs, VMContextBuilder};
use near_sdk::testing_env;

const NEAR: u128 = 1_000_000_000_000_000_000_000_000;
const HOUR: u64 = 3_600_000_000_000;

fn acc(s: &str) -> AccountId {
    s.parse().unwrap()
}

fn ctx(who: &str, deposit: u128, now: u64) {
    let mut b = VMContextBuilder::new();
    b.predecessor_account_id(acc(who))
        .signer_account_id(acc(who))
        .attached_deposit(NearToken::from_yoctonear(deposit))
        .block_timestamp(now)
        .account_balance(NearToken::from_near(10_000));
    testing_env!(b.build());
}

fn setup() -> Actuary {
    ctx("owner.near", 0, 0);
    let mut c = Actuary::new(acc("owner.near"), None, None, None, None, Some(acc("oracle.near")));
    ctx("lp.near", 100 * NEAR, 0);
    c.vault_deposit();
    // verifier.near: a 50 NEAR bond, a 5% fee, Hive reputation 61.20 attested by the oracle.
    ctx("verifier.near", 50 * NEAR, 0);
    c.apply_verifier("vera".into(), "test suites, Rust".into(), vec!["rust".into()], 500);
    ctx("oracle.near", 0, 0);
    c.attest_hive(acc("verifier.near"), "vera".into(), 6_120);
    c
}

/// Buyer posts a job (payment + 5% verifier fee) and the verifier accepts it.
fn post(c: &mut Actuary, agent: &str, payment: u128) -> u64 {
    ctx("buyer.near", payment + payment / 20, 0);
    let id = c.post_job(acc(agent), acc("verifier.near"), "sha256:spec".into(), U128(payment), 24 * HOUR, None);
    ctx("verifier.near", 0, 0);
    c.accept_verification(id);
    id
}

#[test]
fn math_vectors_match_js_engine() {
    // These exact numbers are also asserted in sim/test.js.
    let s = seed_pool(10 * NEAR, 5_000);
    assert_eq!((s.pool_pass, s.pool_fail), (10 * NEAR, 10 * NEAR));
    let s = seed_pool(10 * NEAR, 8_000);
    assert_eq!(s.pool_pass, 2_500_000_000_000_000_000_000_000);
    assert_eq!(s.extra_pass, 7_500_000_000_000_000_000_000_000);
    assert_eq!(price_pass_bps(s.pool_pass, s.pool_fail), 8_000);
    let r = fpmm_buy(10 * NEAR, 10 * NEAR, true, NEAR);
    assert_eq!(r.shares, 1_909_090_909_090_909_090_909_090);
    assert_eq!(price_pass_bps(r.pool_pass, r.pool_fail), 5_475);
    let r = fpmm_buy(10 * NEAR, 10 * NEAR, false, 3 * NEAR);
    assert_eq!(r.shares, 5_307_692_307_692_307_692_307_692);
    assert_eq!(prior_from_record(0, 0), 5_000);
    assert_eq!(prior_from_record(9, 0), 9_090);
    assert_eq!(prior_from_record(0, 30), 500);
    assert_eq!(bayes_bps(5_000, true, 9_200, 3_000), 7_540);
    assert_eq!(bayes_bps(9_000, false, 9_200, 3_000), 5_070);
    let s = seed_pool(10 * NEAR, 5_000);
    let r = reprice_pool(s.pool_pass, s.pool_fail, s.extra_pass, s.extra_fail, 7_540);
    assert_eq!(r.pool_pass, 3_262_599_469_496_021_220_159_151);
    assert_eq!(r.extra_pass, 6_737_400_530_503_978_779_840_849);
    assert_eq!(price_pass_bps(r.pool_pass, r.pool_fail), 7_540);
    let s = seed_pool(10 * NEAR, 9_000);
    let r = reprice_pool(s.pool_pass, s.pool_fail, s.extra_pass, s.extra_fail, 2_000);
    assert_eq!((r.pool_pass, r.pool_fail, r.extra_fail), (10 * NEAR, 2_500_000_000_000_000_000_000_000, 7_500_000_000_000_000_000_000_000));
}

#[test]
fn heartbeats_reprice_the_market_and_keep_money_conserved() {
    let mut c = setup();
    let id = post(&mut c, "nova.near", 10 * NEAR);
    ctx("nova.near", 2 * NEAR, HOUR);
    c.accept_job(id);
    ctx("buyer.near", 3 * NEAR, HOUR);
    let cover = c.buy(id, Outcome::Fail, None).shares.0;
    let p0 = c.pass_price_bps(id);
    ctx("verifier.near", 0, 2 * HOUR);
    let p1 = c.heartbeat(id, true, "sha256:commit".into());
    assert!(p1 > p0, "on-time heartbeat raises PASS");
    let p2 = c.heartbeat(id, false, "missed".into());
    let p3 = c.heartbeat(id, false, "missed".into());
    assert!(p2 < p1 && p3 < p2, "missed heartbeats lower PASS");
    assert_eq!(c.get_job(id).unwrap().missed, 2);
    let before = c.get_vault().total_assets.0;
    ctx("anyone.near", 0, 25 * HOUR + 1);
    c.expire(id);
    let nova = c.get_position(id, acc("nova.near")).fail.0;
    assert_eq!(nova, 0);
    // Collateral in: 10 liquidity + 2 bond + 3 cover. FAIL tokens out: vault + buyer.
    let vault_back = c.get_vault().total_assets.0 - (before - 10 * NEAR);
    assert_eq!(vault_back + cover, 15 * NEAR);
}

#[test]
#[should_panic(expected = "only the verifier can attest heartbeats")]
fn only_verifier_attests_heartbeats() {
    let mut c = setup();
    let id = post(&mut c, "nova.near", NEAR);
    ctx("nova.near", NEAR / 10, HOUR);
    c.accept_job(id);
    ctx("nova.near", 0, HOUR);
    c.heartbeat(id, true, "trust me".into());
}

#[test]
fn new_agent_opens_at_fifty_percent_and_bond_moves_price_up() {
    let mut c = setup();
    let id = post(&mut c, "nova.near", 10 * NEAR);
    assert_eq!(c.pass_price_bps(id), 5_000);
    ctx("nova.near", 2 * NEAR, HOUR);
    let q = c.accept_job(id);
    assert!(q.pass_price_after_bps > 5_000, "bond should raise PASS price");
    assert_eq!(c.get_job(id).unwrap().status, JobStatus::Active);
    assert_eq!(c.get_vault().locked.0, 10 * NEAR);
    assert!(get_logs().iter().any(|l| l.contains("job_accepted")));
}

#[test]
fn buyer_buys_cover_and_is_paid_on_failure() {
    let mut c = setup();
    let id = post(&mut c, "shady.near", 10 * NEAR);
    ctx("shady.near", NEAR, HOUR);
    c.accept_job(id);
    let before = c.pass_price_bps(id);
    ctx("buyer.near", 2 * NEAR, 2 * HOUR);
    let q = c.buy(id, Outcome::Fail, None);
    assert!(c.pass_price_bps(id) < before, "buying cover lowers PASS price");
    // 2 NEAR premium buys more than 2 NEAR of cover because FAIL is priced below 1.
    assert!(q.shares.0 > 2 * NEAR);
    ctx("verifier.near", 0, 3 * HOUR);
    c.resolve(id, Outcome::Fail, "sha256:tests-failed".into());
    let job = c.get_job(id).unwrap();
    assert_eq!(job.status, JobStatus::Failed);
    ctx("buyer.near", 0, 4 * HOUR);
    let paid = c.redeem(id);
    assert_eq!(paid.0, q.shares.0);
    let rec = c.get_agent(acc("shady.near"));
    assert_eq!((rec.jobs, rec.fails), (1, 1));
    assert_eq!(rec.slashed.0, NEAR);
}

#[test]
fn reputation_lowers_premiums_over_time() {
    let mut c = setup();
    for _ in 0..6 {
        let id = post(&mut c, "steady.near", NEAR);
        ctx("steady.near", NEAR / 10, HOUR);
        c.accept_job(id);
        ctx("verifier.near", 0, 2 * HOUR);
        c.resolve(id, Outcome::Pass, "ok".into());
    }
    assert_eq!(c.agent_prior_bps(acc("steady.near")), 8_750);
    let id = post(&mut c, "steady.near", NEAR);
    assert_eq!(c.get_job(id).unwrap().prior_bps, 8_750);
}

#[test]
fn money_is_conserved() {
    let mut c = setup();
    let id = post(&mut c, "nova.near", 10 * NEAR);
    ctx("nova.near", 3 * NEAR, HOUR);
    c.accept_job(id);
    ctx("buyer.near", 2 * NEAR, HOUR);
    let a = c.buy(id, Outcome::Fail, None).shares.0;
    ctx("spec.near", 4 * NEAR, HOUR);
    let b = c.buy(id, Outcome::Pass, None).shares.0;
    let assets_before = c.get_vault().total_assets.0;
    ctx("verifier.near", 0, 2 * HOUR);
    c.resolve(id, Outcome::Pass, "ok".into());
    let v = c.get_vault();
    let nova = c.get_position(id, acc("nova.near")).pass.0;
    // In: liquidity 10 + bond 3 + cover 2 + spec 4 = 19 NEAR, plus the 2% take (0.2 NEAR)
    // on the 10 NEAR payment. Out: vault gets back its winning tokens + fees + take,
    // PASS holders redeem the rest.
    let vault_back = v.total_assets.0 - (assets_before - 10 * NEAR);
    assert_eq!(vault_back + nova + b, 19 * NEAR + NEAR / 5);
    assert_eq!(c.get_position(id, acc("buyer.near")).pass.0, 0);
    let _ = a;
}

#[test]
#[should_panic(expected = "an agent cannot bet against its own work")]
fn agent_cannot_short_itself() {
    let mut c = setup();
    let id = post(&mut c, "nova.near", NEAR);
    ctx("nova.near", NEAR / 10, HOUR);
    c.accept_job(id);
    ctx("nova.near", NEAR, HOUR);
    c.buy(id, Outcome::Fail, None);
}

#[test]
#[should_panic(expected = "the verifier cannot trade")]
fn verifier_cannot_trade() {
    let mut c = setup();
    let id = post(&mut c, "nova.near", NEAR);
    ctx("nova.near", NEAR / 10, HOUR);
    c.accept_job(id);
    ctx("verifier.near", NEAR, HOUR);
    c.buy(id, Outcome::Pass, None);
}

#[test]
#[should_panic(expected = "only the verifier can resolve")]
fn only_verifier_resolves() {
    let mut c = setup();
    let id = post(&mut c, "nova.near", NEAR);
    ctx("nova.near", NEAR / 10, HOUR);
    c.accept_job(id);
    ctx("nova.near", 0, HOUR);
    c.resolve(id, Outcome::Pass, "trust me".into());
}

#[test]
#[should_panic(expected = "bond must be at least")]
fn bond_is_required() {
    let mut c = setup();
    let id = post(&mut c, "nova.near", 10 * NEAR);
    ctx("nova.near", NEAR / 2, HOUR);
    c.accept_job(id);
}

#[test]
fn silent_verifier_loses_its_pledge_and_the_agent_keeps_a_clean_record() {
    let mut c = setup();
    let id = post(&mut c, "ghost.near", 10 * NEAR);
    assert_eq!(c.get_verifier(acc("verifier.near")).unwrap().locked.0, 2 * NEAR);
    ctx("ghost.near", NEAR, HOUR);
    c.accept_job(id);
    ctx("anyone.near", 0, 25 * HOUR + 1);
    c.expire(id);
    let job = c.get_job(id).unwrap();
    assert_eq!(job.status, JobStatus::Failed);
    assert!(job.verifier_timed_out);
    let v = c.get_verifier(acc("verifier.near")).unwrap();
    assert_eq!((v.bond.0, v.locked.0, v.slashed.0, v.timeouts, v.earned.0), (48 * NEAR, 0, 2 * NEAR, 1, 0));
    let rec = c.get_agent(acc("ghost.near"));
    assert_eq!((rec.jobs, rec.fails, rec.unverified), (0, 0, 1));
}

#[test]
fn verifier_is_paid_its_fee_on_either_verdict() {
    let mut c = setup();
    for (agent, outcome) in [("nova.near", Outcome::Pass), ("shady.near", Outcome::Fail)] {
        let id = post(&mut c, agent, 10 * NEAR);
        assert_eq!(c.get_job(id).unwrap().verifier_fee.0, NEAR / 2);
        ctx(agent, NEAR, HOUR);
        c.accept_job(id);
        ctx("verifier.near", 0, 2 * HOUR);
        c.resolve(id, outcome, "sha256:tests".into());
    }
    let v = c.get_verifier(acc("verifier.near")).unwrap();
    assert_eq!((v.passes, v.fails, v.earned.0, v.locked.0, v.accepted), (1, 1, NEAR, 0, 2));
}

#[test]
#[should_panic(expected = "buyer cannot verify its own job")]
fn buyer_cannot_be_the_verifier() {
    let mut c = setup();
    ctx("verifier.near", NEAR, 0);
    c.post_job(acc("nova.near"), acc("verifier.near"), "spec".into(), U128(NEAR), 24 * HOUR, None);
}

#[test]
#[should_panic(expected = "agent cannot verify its own work")]
fn agent_cannot_be_the_verifier() {
    let mut c = setup();
    ctx("buyer.near", NEAR + NEAR / 20, 0);
    c.post_job(acc("verifier.near"), acc("verifier.near"), "spec".into(), U128(NEAR), 24 * HOUR, None);
}

#[test]
#[should_panic(expected = "verifier is not approved")]
fn low_hive_reputation_is_rejected() {
    let mut c = setup();
    ctx("lowrep.near", NEAR, 0);
    c.apply_verifier("newbie".into(), "".into(), vec![], 300);
    ctx("oracle.near", 0, 0);
    assert_eq!(c.attest_hive(acc("lowrep.near"), "newbie".into(), 4_800), VerifierStatus::Rejected);
    ctx("buyer.near", NEAR + NEAR * 3 / 100, 0);
    c.post_job(acc("nova.near"), acc("lowrep.near"), "spec".into(), U128(NEAR), 24 * HOUR, None);
}

#[test]
#[should_panic(expected = "only the Hive oracle can attest")]
fn only_the_oracle_attests_hive() {
    let mut c = setup();
    ctx("lowrep.near", NEAR, 0);
    c.apply_verifier("newbie".into(), "".into(), vec![], 300);
    c.attest_hive(acc("lowrep.near"), "newbie".into(), 9_000);
}

#[test]
#[should_panic(expected = "the verifier has not accepted this job yet")]
fn agent_waits_for_the_verifier() {
    let mut c = setup();
    ctx("buyer.near", NEAR + NEAR / 20, 0);
    let id = c.post_job(acc("nova.near"), acc("verifier.near"), "spec".into(), U128(NEAR), 24 * HOUR, None);
    ctx("nova.near", NEAR / 10, HOUR);
    c.accept_job(id);
}

#[test]
fn verifier_declines_and_buyer_is_refunded() {
    let mut c = setup();
    ctx("buyer.near", NEAR + NEAR / 20, 0);
    let id = c.post_job(acc("nova.near"), acc("verifier.near"), "spec".into(), U128(NEAR), 24 * HOUR, None);
    ctx("verifier.near", 0, 0);
    c.decline_verification(id);
    assert_eq!(c.get_job(id).unwrap().status, JobStatus::Cancelled);
    assert_eq!(c.get_verifier(acc("verifier.near")).unwrap().declined, 1);
}

#[test]
fn agents_list_profiles_with_their_record() {
    let mut c = setup();
    ctx("nova.near", LISTING_DEPOSIT, 0);
    c.list_agent("Nova".into(), vec!["rust".into(), "near contracts".into()], vec!["claude-opus-5-5".into()], "Writes and tests NEAR contracts.".into(), "https://nova.example/jobs".into(), U128(5 * NEAR));
    let id = post(&mut c, "nova.near", NEAR);
    ctx("nova.near", NEAR / 10, HOUR);
    c.accept_job(id);
    ctx("verifier.near", 0, 2 * HOUR);
    c.resolve(id, Outcome::Pass, "ok".into());
    let l = c.list_agent_listings(None, None);
    assert_eq!(l.len(), 1);
    assert_eq!(l[0].profile.models, vec!["claude-opus-5-5".to_string()]);
    assert_eq!((l[0].record.passes, l[0].prior_bps), (1, 6_666));
}

#[test]
#[should_panic(expected = "too early to expire")]
fn cannot_expire_early() {
    let mut c = setup();
    let id = post(&mut c, "ghost.near", NEAR);
    ctx("ghost.near", NEAR / 10, HOUR);
    c.accept_job(id);
    ctx("anyone.near", 0, 24 * HOUR + 5);
    c.expire(id);
}

#[test]
fn vault_withdraw_only_free_capital() {
    let mut c = setup();
    let id = post(&mut c, "nova.near", 60 * NEAR);
    ctx("nova.near", 6 * NEAR, HOUR);
    c.accept_job(id);
    assert_eq!(c.get_vault().free.0, 40 * NEAR);
    ctx("lp.near", 0, HOUR);
    let _ = c.vault_withdraw(U128(30 * NEAR));
    assert_eq!(c.get_vault().free.0, 10 * NEAR);
}

#[test]
#[should_panic(expected = "capital is locked")]
fn vault_cannot_withdraw_locked() {
    let mut c = setup();
    let id = post(&mut c, "nova.near", 60 * NEAR);
    ctx("nova.near", 6 * NEAR, HOUR);
    c.accept_job(id);
    ctx("lp.near", 0, HOUR);
    let _ = c.vault_withdraw(U128(50 * NEAR));
}
