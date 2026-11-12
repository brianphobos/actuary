//! Actuary: a market that insures AI agent work.
//!
//! Every job an AI agent takes on gets its own two-outcome market: PASS (the work
//! is delivered and verified) or FAIL. The market price of PASS is a live,
//! money-backed estimate of whether the agent will deliver. Three things fall out
//! of that one market:
//!
//! * **Insurance.** A buyer who holds FAIL shares is paid 1 yoctoNEAR per share
//!   if the job fails. Buying FAIL is buying cover, and the price is the premium.
//! * **A performance bond.** The agent must stake on PASS to accept the job. It
//!   loses that stake if it fails, so it has skin in the game.
//! * **Reputation.** The opening price is set from the agent's on-chain record,
//!   and every resolved job updates that record. New agents with no history can
//!   still get hired by bonding more, which makes cover cheap for the buyer.
//!
//! Liquidity comes from an underwriting vault. Liquidity providers deposit NEAR
//! once; the contract seeds every new job's market from the vault, and the vault
//! earns the trading fees across all jobs. This is the insurance pool.
//!
//! The market maker is a Fixed Product Market Maker (the same family as the
//! Gnosis/Polymarket FPMM), which needs only integer multiply and divide.

use near_sdk::json_types::U128;
use near_sdk::store::{LookupMap, Vector};
use near_sdk::{env, near, require, AccountId, BorshStorageKey, NearToken, PanicOnDefault, Promise};

mod math;
pub use math::*;

pub const BPS: u128 = 10_000;
/// Smallest trade or bond, so nobody can bloat storage with dust positions.
pub const MIN_TRADE: u128 = 10_000_000_000_000_000_000_000; // 0.01 NEAR
/// Grace period after the deadline before anyone may expire a job as FAIL.
pub const DEFAULT_GRACE_NS: u64 = 60 * 60 * 1_000_000_000; // 1 hour

#[derive(BorshStorageKey)]
#[near]
enum StorageKey {
    Jobs,
    Positions,
    Agents,
    VaultShares,
}

#[near(serializers = [borsh, json])]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Pass,
    Fail,
}

#[near(serializers = [borsh, json])]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobStatus {
    /// Created by the buyer, waiting for the agent to post its bond.
    Posted,
    /// Agent bonded, market open, work in progress.
    Active,
    Passed,
    Failed,
    /// Agent never accepted before the deadline; buyer refunded, vault made whole.
    Cancelled,
}

#[near(serializers = [borsh, json])]
#[derive(Clone, Debug)]
pub struct Job {
    pub id: u64,
    pub buyer: AccountId,
    pub agent: AccountId,
    pub verifier: AccountId,
    /// Hash or URL of the job spec, so the verifier and traders can read it.
    pub spec: String,
    /// What the buyer pays the agent on PASS. Held in escrow.
    pub payment: U128,
    pub deadline_ns: u64,
    pub grace_ns: u64,
    pub status: JobStatus,
    /// Opening PASS probability, in basis points, priced from the agent's record.
    pub prior_bps: u32,
    /// The agent's stake on PASS.
    pub bond: U128,
    /// Collateral the vault put in to seed the market.
    pub liquidity: U128,
    pub pool_pass: U128,
    pub pool_fail: U128,
    /// Outcome tokens the vault holds outside the pool after seeding at a skewed price.
    pub vault_extra_pass: U128,
    pub vault_extra_fail: U128,
    pub fees: U128,
    /// PASS price when the job was resolved, for the agent's calibration record.
    pub closing_bps: u32,
    pub evidence: Option<String>,
    pub created_ns: u64,
    pub heartbeats: u32,
    pub missed: u32,
}

#[near(serializers = [borsh, json])]
#[derive(Clone, Debug, Default)]
pub struct Position {
    pub pass: U128,
    pub fail: U128,
}

#[near(serializers = [borsh, json])]
#[derive(Clone, Debug, Default)]
pub struct AgentRecord {
    pub jobs: u32,
    pub passes: u32,
    pub fails: u32,
    /// Sum of opening PASS probabilities (bps) of resolved jobs.
    pub sum_prior_bps: u64,
    /// Sum of squared errors of the closing price against the outcome, in bps^2 / 10_000.
    /// Divide by jobs to get a Brier score in bps: lower means the market read this agent well.
    pub brier_bps_sum: u64,
    /// Total bond the agent has had at stake, and total it has lost.
    pub bonded: U128,
    pub slashed: U128,
    pub earned: U128,
}

#[near(serializers = [json])]
pub struct Quote {
    pub shares: U128,
    pub fee: U128,
    /// Average price paid per share, in bps.
    pub avg_price_bps: u32,
    /// PASS price after the trade, in bps.
    pub pass_price_after_bps: u32,
}

#[near(serializers = [json])]
pub struct VaultView {
    pub total_assets: U128,
    pub free: U128,
    pub locked: U128,
    pub total_shares: U128,
    pub realized_pnl: String,
    pub fees_earned: U128,
}

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct Actuary {
    owner: AccountId,
    jobs: Vector<Job>,
    positions: LookupMap<String, Position>,
    agents: LookupMap<AccountId, AgentRecord>,
    vault_shares: LookupMap<AccountId, u128>,
    vault_total_shares: u128,
    vault_free: u128,
    vault_locked: u128,
    vault_fees_earned: u128,
    /// Signed P&L of the vault so far (can be negative).
    vault_pnl: i128,
    /// Trading fee in bps, paid to the vault.
    fee_bps: u32,
    /// Liquidity seeded per job, as bps of the job payment.
    liquidity_bps: u32,
    /// Minimum bond, as bps of the job payment.
    min_bond_bps: u32,
    /// Underwriting take on a passed job's payment, paid to the vault.
    take_bps: u32,
    /// P(heartbeat on time | job will pass) and P(on time | job will fail), in bps.
    hb_pass_bps: u32,
    hb_fail_bps: u32,
}

fn pos_key(job_id: u64, account: &AccountId) -> String {
    format!("{job_id}:{account}")
}

fn yocto(x: u128) -> NearToken {
    NearToken::from_yoctonear(x)
}

#[near]
impl Actuary {
    #[init]
    pub fn new(owner: AccountId, fee_bps: Option<u32>, liquidity_bps: Option<u32>, min_bond_bps: Option<u32>, take_bps: Option<u32>) -> Self {
        let fee_bps = fee_bps.unwrap_or(300);
        let take_bps = take_bps.unwrap_or(200);
        require!(fee_bps <= 1_000 && take_bps <= 1_000, "fee too high");
        Self {
            owner,
            jobs: Vector::new(StorageKey::Jobs),
            positions: LookupMap::new(StorageKey::Positions),
            agents: LookupMap::new(StorageKey::Agents),
            vault_shares: LookupMap::new(StorageKey::VaultShares),
            vault_total_shares: 0,
            vault_free: 0,
            vault_locked: 0,
            vault_fees_earned: 0,
            vault_pnl: 0,
            fee_bps,
            liquidity_bps: liquidity_bps.unwrap_or(10_000),
            min_bond_bps: min_bond_bps.unwrap_or(1_000),
            take_bps,
            hb_pass_bps: 9_200,
            hb_fail_bps: 3_000,
        }
    }

    // ------------------------------------------------------------------
    // Underwriting vault
    // ------------------------------------------------------------------

    /// Deposit NEAR into the underwriting vault and receive vault shares.
    #[payable]
    pub fn vault_deposit(&mut self) -> U128 {
        let amount = env::attached_deposit().as_yoctonear();
        require!(amount >= MIN_TRADE, "deposit too small");
        let assets = self.vault_total_assets();
        let minted = if self.vault_total_shares == 0 || assets == 0 {
            amount
        } else {
            mul_div(amount, self.vault_total_shares, assets)
        };
        let who = env::predecessor_account_id();
        let cur = self.vault_shares.get(&who).copied().unwrap_or(0);
        self.vault_shares.insert(who, cur + minted);
        self.vault_total_shares += minted;
        self.vault_free += amount;
        U128(minted)
    }

    /// Burn vault shares for NEAR. Only capital not locked in open jobs can leave.
    pub fn vault_withdraw(&mut self, shares: U128) -> Promise {
        let who = env::predecessor_account_id();
        let cur = self.vault_shares.get(&who).copied().unwrap_or(0);
        require!(shares.0 > 0 && shares.0 <= cur, "not enough vault shares");
        let amount = mul_div(shares.0, self.vault_total_assets(), self.vault_total_shares);
        require!(amount <= self.vault_free, "capital is locked in open jobs; try later");
        self.vault_shares.insert(who.clone(), cur - shares.0);
        self.vault_total_shares -= shares.0;
        self.vault_free -= amount;
        Promise::new(who).transfer(yocto(amount))
    }

    // ------------------------------------------------------------------
    // Job lifecycle
    // ------------------------------------------------------------------

    /// Buyer posts a job and escrows the payment (the attached deposit).
    /// The market opens when the agent accepts by posting its bond.
    #[payable]
    pub fn post_job(&mut self, agent: AccountId, verifier: AccountId, spec: String, deadline_ns: u64, grace_ns: Option<u64>) -> u64 {
        let payment = env::attached_deposit().as_yoctonear();
        let buyer = env::predecessor_account_id();
        require!(payment >= MIN_TRADE, "payment too small");
        require!(agent != buyer, "buyer cannot hire itself");
        require!(verifier != agent, "agent cannot verify its own work");
        require!(deadline_ns > env::block_timestamp(), "deadline is in the past");
        require!(spec.len() <= 512, "spec too long; pass a hash or URL");
        let rec = self.agents.get(&agent).cloned().unwrap_or_default();
        let id = self.jobs.len() as u64;
        self.jobs.push(Job {
            id,
            buyer,
            agent,
            verifier,
            spec,
            payment: U128(payment),
            deadline_ns,
            grace_ns: grace_ns.unwrap_or(DEFAULT_GRACE_NS),
            status: JobStatus::Posted,
            prior_bps: prior_from_record(rec.passes, rec.fails),
            bond: U128(0),
            liquidity: U128(0),
            pool_pass: U128(0),
            pool_fail: U128(0),
            vault_extra_pass: U128(0),
            vault_extra_fail: U128(0),
            fees: U128(0),
            closing_bps: 0,
            evidence: None,
            created_ns: env::block_timestamp(),
            heartbeats: 0,
            missed: 0,
        });
        env::log_str(&format!("EVENT_JSON:{{\"event\":\"job_posted\",\"job\":{id}}}"));
        id
    }

    /// The hired agent accepts the job by staking its bond on PASS (attached deposit).
    /// This seeds the job's market from the vault at the agent's reputation price,
    /// then spends the bond buying PASS, which moves the price up.
    #[payable]
    pub fn accept_job(&mut self, job_id: u64) -> Quote {
        let bond = env::attached_deposit().as_yoctonear();
        let mut job = self.job_mut(job_id);
        require!(env::predecessor_account_id() == job.agent, "only the hired agent can accept");
        require!(job.status == JobStatus::Posted, "job is not waiting for acceptance");
        require!(env::block_timestamp() < job.deadline_ns, "deadline passed");
        let min_bond = (job.payment.0 * self.min_bond_bps as u128 / BPS).max(MIN_TRADE);
        require!(bond >= min_bond, format!("bond must be at least {min_bond} yoctoNEAR"));

        // Seed the market from the vault.
        let liquidity = job.payment.0 * self.liquidity_bps as u128 / BPS;
        require!(liquidity <= self.vault_free, "underwriting vault has too little free capital");
        self.vault_free -= liquidity;
        self.vault_locked += liquidity;
        let seed = seed_pool(liquidity, job.prior_bps);
        job.liquidity = U128(liquidity);
        job.pool_pass = U128(seed.pool_pass);
        job.pool_fail = U128(seed.pool_fail);
        job.vault_extra_pass = U128(seed.extra_pass);
        job.vault_extra_fail = U128(seed.extra_fail);
        job.status = JobStatus::Active;
        job.bond = U128(bond);

        let quote = self.execute_buy(&mut job, Outcome::Pass, bond, 0);
        let mut rec = self.agents.get(&job.agent).cloned().unwrap_or_default();
        rec.bonded = U128(rec.bonded.0 + bond);
        self.agents.insert(job.agent.clone(), rec);
        self.jobs.replace(job_id as u32, job);
        env::log_str(&format!("EVENT_JSON:{{\"event\":\"job_accepted\",\"job\":{job_id},\"pass_bps\":{}}}", quote.pass_price_after_bps));
        quote
    }

    /// Buy PASS or FAIL shares on an active job. Each winning share pays 1 yoctoNEAR.
    /// Buying FAIL is buying insurance on the job.
    #[payable]
    pub fn buy(&mut self, job_id: u64, outcome: Outcome, min_shares: Option<U128>) -> Quote {
        let amount = env::attached_deposit().as_yoctonear();
        require!(amount >= MIN_TRADE, "trade too small");
        let who = env::predecessor_account_id();
        let mut job = self.job_mut(job_id);
        require!(job.status == JobStatus::Active, "market is not open");
        require!(env::block_timestamp() < job.deadline_ns, "market closed at the deadline");
        require!(who != job.verifier, "the verifier cannot trade on jobs it judges");
        require!(!(who == job.agent && outcome == Outcome::Fail), "an agent cannot bet against its own work");
        let q = self.execute_buy(&mut job, outcome, amount, min_shares.map(|m| m.0).unwrap_or(0));
        self.jobs.replace(job_id as u32, job);
        q
    }

    /// The verifier attests a progress heartbeat (a milestone delivered on time, or missed).
    /// The vault's market maker reprices itself with a Bayes update, so public progress
    /// news moves the quote directly instead of being a free option for the fastest trader.
    /// Returns the new PASS price in bps.
    pub fn heartbeat(&mut self, job_id: u64, on_time: bool, evidence: String) -> u32 {
        let mut job = self.job_mut(job_id);
        require!(env::predecessor_account_id() == job.verifier, "only the verifier can attest heartbeats");
        require!(job.status == JobStatus::Active, "job is not active");
        require!(env::block_timestamp() < job.deadline_ns, "market closed at the deadline");
        require!(evidence.len() <= 512, "evidence too long; pass a hash or URL");
        let before = price_pass_bps(job.pool_pass.0, job.pool_fail.0);
        let target = bayes_bps(before, on_time, self.hb_pass_bps, self.hb_fail_bps);
        let s = reprice_pool(job.pool_pass.0, job.pool_fail.0, job.vault_extra_pass.0, job.vault_extra_fail.0, target);
        job.pool_pass = U128(s.pool_pass);
        job.pool_fail = U128(s.pool_fail);
        job.vault_extra_pass = U128(s.extra_pass);
        job.vault_extra_fail = U128(s.extra_fail);
        job.heartbeats += 1;
        if !on_time {
            job.missed += 1;
        }
        let after = price_pass_bps(s.pool_pass, s.pool_fail);
        env::log_str(&format!(
            "EVENT_JSON:{{\"event\":\"heartbeat\",\"job\":{job_id},\"on_time\":{on_time},\"pass_bps_before\":{before},\"pass_bps\":{after}}}"
        ));
        self.jobs.replace(job_id as u32, job);
        after
    }

    /// The verifier rules on the delivered work. Before the deadline + grace only;
    /// after that anyone can call `expire`.
    pub fn resolve(&mut self, job_id: u64, outcome: Outcome, evidence: String) {
        let job = self.job_mut(job_id);
        require!(env::predecessor_account_id() == job.verifier, "only the verifier can resolve");
        require!(job.status == JobStatus::Active, "job is not active");
        require!(evidence.len() <= 512, "evidence too long; pass a hash or URL");
        self.settle(job, outcome, Some(evidence));
    }

    /// Anyone may close a job that the verifier never ruled on. An active job
    /// past its deadline + grace is ruled FAIL (no verified delivery). A posted job
    /// the agent never accepted is cancelled and the buyer refunded.
    pub fn expire(&mut self, job_id: u64) {
        let mut job = self.job_mut(job_id);
        require!(env::block_timestamp() >= job.deadline_ns + job.grace_ns, "too early to expire");
        match job.status {
            JobStatus::Active => self.settle(job, Outcome::Fail, Some("expired: no verified delivery".into())),
            JobStatus::Posted => {
                job.status = JobStatus::Cancelled;
                let _ = Promise::new(job.buyer.clone()).transfer(yocto(job.payment.0));
                self.jobs.replace(job_id as u32, job);
            }
            _ => env::panic_str("job already closed"),
        }
    }

    /// Pay out a trader's winning shares on a resolved job.
    pub fn redeem(&mut self, job_id: u64) -> U128 {
        let who = env::predecessor_account_id();
        let job = self.job(job_id);
        let key = pos_key(job_id, &who);
        let pos = self.positions.get(&key).cloned().unwrap_or_default();
        let payout = match job.status {
            JobStatus::Passed => pos.pass.0,
            JobStatus::Failed => pos.fail.0,
            _ => env::panic_str("job not resolved"),
        };
        require!(payout > 0, "nothing to redeem");
        self.positions.remove(&key);
        let _ = Promise::new(who).transfer(yocto(payout));
        U128(payout)
    }

    // ------------------------------------------------------------------
    // Views
    // ------------------------------------------------------------------

    pub fn get_job(&self, job_id: u64) -> Option<Job> {
        self.jobs.get(job_id as u32).cloned()
    }

    pub fn list_jobs(&self, from: Option<u64>, limit: Option<u64>) -> Vec<Job> {
        let from = from.unwrap_or(0);
        let limit = limit.unwrap_or(50).min(100);
        (from..(from + limit).min(self.jobs.len() as u64))
            .filter_map(|i| self.jobs.get(i as u32).cloned())
            .collect()
    }

    /// PASS price in bps (= market-implied probability the agent delivers).
    pub fn pass_price_bps(&self, job_id: u64) -> u32 {
        let j = self.job(job_id);
        match j.status {
            JobStatus::Posted => j.prior_bps,
            _ => price_pass_bps(j.pool_pass.0, j.pool_fail.0),
        }
    }

    /// What `buy` would return for `amount`, without trading.
    pub fn quote(&self, job_id: u64, outcome: Outcome, amount: U128) -> Quote {
        let j = self.job(job_id);
        require!(j.status == JobStatus::Active, "market is not open");
        let fee = amount.0 * self.fee_bps as u128 / BPS;
        let r = fpmm_buy(j.pool_pass.0, j.pool_fail.0, outcome == Outcome::Pass, amount.0 - fee);
        Quote {
            shares: U128(r.shares),
            fee: U128(fee),
            avg_price_bps: (mul_div(amount.0, BPS, r.shares.max(1))) as u32,
            pass_price_after_bps: price_pass_bps(r.pool_pass, r.pool_fail),
        }
    }

    pub fn get_position(&self, job_id: u64, account: AccountId) -> Position {
        self.positions.get(&pos_key(job_id, &account)).cloned().unwrap_or_default()
    }

    pub fn get_agent(&self, agent: AccountId) -> AgentRecord {
        self.agents.get(&agent).cloned().unwrap_or_default()
    }

    /// Opening PASS probability a new job for this agent would get.
    pub fn agent_prior_bps(&self, agent: AccountId) -> u32 {
        let r = self.agents.get(&agent).cloned().unwrap_or_default();
        prior_from_record(r.passes, r.fails)
    }

    pub fn get_vault(&self) -> VaultView {
        VaultView {
            total_assets: U128(self.vault_total_assets()),
            free: U128(self.vault_free),
            locked: U128(self.vault_locked),
            total_shares: U128(self.vault_total_shares),
            realized_pnl: self.vault_pnl.to_string(),
            fees_earned: U128(self.vault_fees_earned),
        }
    }

    pub fn get_vault_shares(&self, account: AccountId) -> U128 {
        U128(self.vault_shares.get(&account).copied().unwrap_or(0))
    }

    /// (owner, fee_bps, liquidity_bps, min_bond_bps, take_bps, hb_pass_bps, hb_fail_bps)
    pub fn get_config(&self) -> (AccountId, u32, u32, u32, u32, u32, u32) {
        (self.owner.clone(), self.fee_bps, self.liquidity_bps, self.min_bond_bps, self.take_bps, self.hb_pass_bps, self.hb_fail_bps)
    }
}

impl Actuary {
    fn job(&self, job_id: u64) -> &Job {
        self.jobs.get(job_id as u32).unwrap_or_else(|| env::panic_str("no such job"))
    }

    fn job_mut(&self, job_id: u64) -> Job {
        self.job(job_id).clone()
    }

    fn vault_total_assets(&self) -> u128 {
        self.vault_free + self.vault_locked
    }

    fn execute_buy(&mut self, job: &mut Job, outcome: Outcome, amount: u128, min_shares: u128) -> Quote {
        let who = env::predecessor_account_id();
        let fee = amount * self.fee_bps as u128 / BPS;
        let invest = amount - fee;
        let r = fpmm_buy(job.pool_pass.0, job.pool_fail.0, outcome == Outcome::Pass, invest);
        require!(r.shares >= min_shares, "slippage: fewer shares than min_shares");
        job.pool_pass = U128(r.pool_pass);
        job.pool_fail = U128(r.pool_fail);
        job.fees = U128(job.fees.0 + fee);
        let key = pos_key(job.id, &who);
        let mut pos = self.positions.get(&key).cloned().unwrap_or_default();
        match outcome {
            Outcome::Pass => pos.pass = U128(pos.pass.0 + r.shares),
            Outcome::Fail => pos.fail = U128(pos.fail.0 + r.shares),
        }
        self.positions.insert(key, pos);
        let after = price_pass_bps(r.pool_pass, r.pool_fail);
        env::log_str(&format!(
            "EVENT_JSON:{{\"event\":\"trade\",\"job\":{},\"trader\":\"{}\",\"outcome\":\"{:?}\",\"amount\":\"{}\",\"shares\":\"{}\",\"pass_bps\":{}}}",
            job.id, who, outcome, amount, r.shares, after
        ));
        Quote {
            shares: U128(r.shares),
            fee: U128(fee),
            avg_price_bps: mul_div(amount, BPS, r.shares.max(1)) as u32,
            pass_price_after_bps: after,
        }
    }

    fn settle(&mut self, mut job: Job, outcome: Outcome, evidence: Option<String>) {
        let closing = price_pass_bps(job.pool_pass.0, job.pool_fail.0);
        job.closing_bps = closing;
        job.evidence = evidence;
        job.status = if outcome == Outcome::Pass { JobStatus::Passed } else { JobStatus::Failed };

        // Escrow: on PASS the agent is paid minus the underwriting take; on FAIL the buyer is refunded in full.
        let take = if outcome == Outcome::Pass { job.payment.0 * self.take_bps as u128 / BPS } else { 0 };
        let payee = if outcome == Outcome::Pass { job.agent.clone() } else { job.buyer.clone() };
        let _ = Promise::new(payee).transfer(yocto(job.payment.0 - take));

        // Vault: gets back its winning outcome tokens (pool + extra) plus fees.
        let vault_back = match outcome {
            Outcome::Pass => job.pool_pass.0 + job.vault_extra_pass.0,
            Outcome::Fail => job.pool_fail.0 + job.vault_extra_fail.0,
        } + job.fees.0
            + take;
        self.vault_locked -= job.liquidity.0;
        self.vault_free += vault_back;
        self.vault_fees_earned += job.fees.0 + take;
        self.vault_pnl += vault_back as i128 - job.liquidity.0 as i128;

        // Agent record (reputation).
        let mut rec = self.agents.get(&job.agent).cloned().unwrap_or_default();
        rec.jobs += 1;
        rec.sum_prior_bps += job.prior_bps as u64;
        let target: i64 = if outcome == Outcome::Pass { 10_000 } else { 0 };
        let err = closing as i64 - target;
        rec.brier_bps_sum += (err * err / 10_000) as u64;
        if outcome == Outcome::Pass {
            rec.passes += 1;
            rec.earned = U128(rec.earned.0 + job.payment.0 - take);
        } else {
            rec.fails += 1;
            rec.slashed = U128(rec.slashed.0 + job.bond.0);
        }
        self.agents.insert(job.agent.clone(), rec);
        env::log_str(&format!(
            "EVENT_JSON:{{\"event\":\"job_resolved\",\"job\":{},\"outcome\":\"{:?}\",\"closing_pass_bps\":{}}}",
            job.id, outcome, closing
        ));
        let id = job.id;
        self.jobs.replace(id as u32, job);
    }
}

#[cfg(test)]
mod tests;
