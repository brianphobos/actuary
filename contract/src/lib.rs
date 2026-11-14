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
//! Two registries sit around the market:
//!
//! * **Agent marketplace.** Any account can list itself as an agent: a name, what
//!   it specialises in, which AI models run behind it, and how to reach it. Its
//!   PASS/FAIL record comes from the jobs it has actually done here.
//! * **Verifiers.** A verifier applies with a NEAR bond and a Hive account. A Hive
//!   oracle account checks the Hive reputation off chain (Hive Keychain or
//!   HiveSigner proves the applicant controls the account) and approves it on
//!   chain if the score is at least 55. Only approved verifiers can be named on a
//!   job. They are paid a fee on every ruling, PASS or FAIL, so the verdict does
//!   not change their pay. When they accept a job they pledge part of their bond,
//!   which they lose to the agent and the buyer if they never rule.
//!
//! The market maker is a Fixed Product Market Maker (the same family as the
//! Gnosis/Polymarket FPMM), which needs only integer multiply and divide.

use near_sdk::json_types::U128;
use near_sdk::store::{LookupMap, Vector};
use near_sdk::{env, near, require, AccountId, BorshStorageKey, NearToken, PanicOnDefault, Promise};

mod math;
pub use math::*;

pub const BPS: u128 = 10_000;
const NEAR_1: u128 = 1_000_000_000_000_000_000_000_000;
/// Smallest trade or bond, so nobody can bloat storage with dust positions.
pub const MIN_TRADE: u128 = 10_000_000_000_000_000_000_000; // 0.01 NEAR
/// Grace period after the deadline before anyone may expire a job as FAIL.
pub const DEFAULT_GRACE_NS: u64 = 60 * 60 * 1_000_000_000; // 1 hour
/// Deposit to list an agent profile (covers its storage).
pub const LISTING_DEPOSIT: u128 = 50_000_000_000_000_000_000_000; // 0.05 NEAR
/// Highest fee a verifier may charge, in bps of the job payment.
pub const MAX_VERIFIER_FEE_BPS: u32 = 2_000;

#[derive(BorshStorageKey)]
#[near]
enum StorageKey {
    Jobs,
    Positions,
    Agents,
    VaultShares,
    Profiles,
    ProfileList,
    Verifiers,
    VerifierList,
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
    /// Paid to the verifier when it rules, whatever the verdict. Escrowed with the payment.
    pub verifier_fee: U128,
    /// Bond the verifier locked when it accepted the job. Lost if it never rules.
    pub verifier_pledge: U128,
    pub verifier_accepted: bool,
    /// The verifier never ruled and the job was expired.
    pub verifier_timed_out: bool,
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
    /// Jobs closed because the verifier never ruled. Not counted as passes or fails.
    pub unverified: u32,
}

/// What an agent advertises in the marketplace.
#[near(serializers = [borsh, json])]
#[derive(Clone, Debug, Default)]
pub struct AgentProfile {
    pub name: String,
    /// What it is good at, e.g. "rust", "smart-contract audit", "data cleaning".
    pub specialties: Vec<String>,
    /// The AI models running behind the agent, e.g. "claude-opus-5-5".
    pub models: Vec<String>,
    pub description: String,
    /// Where buyers send the spec and the agent sends its work (URL).
    pub endpoint: String,
    /// The agent's asking price per job, in yoctoNEAR (a guide; the buyer sets the payment).
    pub rate: U128,
    pub listed_ns: u64,
}

#[near(serializers = [json])]
pub struct AgentListing {
    pub account: AccountId,
    pub profile: AgentProfile,
    pub record: AgentRecord,
    /// Opening PASS price a new job for this agent would get.
    pub prior_bps: u32,
}

#[near(serializers = [borsh, json])]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerifierStatus {
    /// Applied; waiting for the Hive oracle to check the reputation.
    Pending,
    /// Hive reputation checked; may be named on jobs.
    Approved,
    /// Hive reputation below the minimum.
    Rejected,
    /// Withdrew its whole bond.
    Retired,
}

#[near(serializers = [borsh, json])]
#[derive(Clone, Debug)]
pub struct Verifier {
    pub account: AccountId,
    pub hive_account: String,
    /// Hive reputation score x 100 at the last attestation (55.00 = 5500).
    pub hive_reputation: u32,
    pub status: VerifierStatus,
    pub description: String,
    pub specialties: Vec<String>,
    /// Fee per job, in bps of the job payment.
    pub fee_bps: u32,
    pub bond: U128,
    /// Part of the bond pledged to jobs it has accepted and not yet ruled on.
    pub locked: U128,
    pub accepted: u32,
    pub declined: u32,
    pub passes: u32,
    pub fails: u32,
    pub timeouts: u32,
    pub earned: U128,
    pub slashed: U128,
    pub applied_ns: u64,
    pub attested_ns: u64,
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
    profiles: LookupMap<AccountId, AgentProfile>,
    profile_list: Vector<AccountId>,
    verifiers: LookupMap<AccountId, Verifier>,
    verifier_list: Vector<AccountId>,
    /// Account that attests Hive reputations on chain.
    hive_oracle: AccountId,
    /// Minimum Hive reputation x 100 to be approved as a verifier.
    min_hive_rep: u32,
    /// Minimum verifier bond.
    min_verifier_bond: u128,
    /// Bond a verifier pledges when it accepts a job, in bps of the job payment.
    verifier_pledge_bps: u32,
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
    pub fn new(owner: AccountId, fee_bps: Option<u32>, liquidity_bps: Option<u32>, min_bond_bps: Option<u32>, take_bps: Option<u32>, hive_oracle: Option<AccountId>) -> Self {
        let fee_bps = fee_bps.unwrap_or(300);
        let take_bps = take_bps.unwrap_or(200);
        require!(fee_bps <= 1_000 && take_bps <= 1_000, "fee too high");
        Self {
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
            profiles: LookupMap::new(StorageKey::Profiles),
            profile_list: Vector::new(StorageKey::ProfileList),
            verifiers: LookupMap::new(StorageKey::Verifiers),
            verifier_list: Vector::new(StorageKey::VerifierList),
            hive_oracle: hive_oracle.unwrap_or_else(|| owner.clone()),
            min_hive_rep: 5_500,
            min_verifier_bond: NEAR_1,
            verifier_pledge_bps: 2_000,
            owner,
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

    /// Buyer posts a job and escrows the payment plus the verifier's fee (the attached
    /// deposit must equal `payment + job_cost(verifier, payment).fee`). The verifier must be
    /// an approved verifier and cannot be the agent or the buyer. The verifier then accepts
    /// or declines, and the market opens when the agent accepts by posting its bond.
    #[payable]
    pub fn post_job(&mut self, agent: AccountId, verifier: AccountId, spec: String, payment: U128, deadline_ns: u64, grace_ns: Option<u64>) -> u64 {
        let payment = payment.0;
        let buyer = env::predecessor_account_id();
        require!(payment >= MIN_TRADE, "payment too small");
        require!(agent != buyer, "buyer cannot hire itself");
        require!(verifier != agent, "agent cannot verify its own work");
        require!(verifier != buyer, "buyer cannot verify its own job");
        let v = self.verifiers.get(&verifier).unwrap_or_else(|| env::panic_str("verifier is not registered"));
        require!(v.status == VerifierStatus::Approved, "verifier is not approved");
        let verifier_fee = payment * v.fee_bps as u128 / BPS;
        require!(env::attached_deposit().as_yoctonear() == payment + verifier_fee, format!("attach payment + verifier fee = {} yoctoNEAR", payment + verifier_fee));
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
            verifier_fee: U128(verifier_fee),
            verifier_pledge: U128(0),
            verifier_accepted: false,
            verifier_timed_out: false,
        });
        env::log_str(&format!("EVENT_JSON:{{\"event\":\"job_posted\",\"job\":{id}}}"));
        id
    }

    /// The named verifier agrees to judge the job and pledges part of its bond to it.
    pub fn accept_verification(&mut self, job_id: u64) -> U128 {
        let mut job = self.job_mut(job_id);
        let who = env::predecessor_account_id();
        require!(who == job.verifier, "only the named verifier can accept");
        require!(job.status == JobStatus::Posted && !job.verifier_accepted, "job is not waiting for its verifier");
        let mut v = self.verifier_of(&who);
        require!(v.status == VerifierStatus::Approved, "verifier is not approved");
        let pledge = job.payment.0 * self.verifier_pledge_bps as u128 / BPS;
        require!(v.bond.0 - v.locked.0 >= pledge, format!("pledge needs {pledge} yoctoNEAR of free verifier bond"));
        v.locked = U128(v.locked.0 + pledge);
        v.accepted += 1;
        self.verifiers.insert(who, v);
        job.verifier_pledge = U128(pledge);
        job.verifier_accepted = true;
        self.jobs.replace(job_id as u32, job);
        env::log_str(&format!("EVENT_JSON:{{\"event\":\"verification_accepted\",\"job\":{job_id}}}"));
        U128(pledge)
    }

    /// The named verifier turns the job down. The buyer is refunded in full.
    pub fn decline_verification(&mut self, job_id: u64) {
        let mut job = self.job_mut(job_id);
        let who = env::predecessor_account_id();
        require!(who == job.verifier, "only the named verifier can decline");
        require!(job.status == JobStatus::Posted && !job.verifier_accepted, "job is not waiting for its verifier");
        let mut v = self.verifier_of(&who);
        v.declined += 1;
        self.verifiers.insert(who, v);
        job.status = JobStatus::Cancelled;
        let _ = Promise::new(job.buyer.clone()).transfer(yocto(job.payment.0 + job.verifier_fee.0));
        self.jobs.replace(job_id as u32, job);
        env::log_str(&format!("EVENT_JSON:{{\"event\":\"verification_declined\",\"job\":{job_id}}}"));
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
        require!(job.verifier_accepted, "the verifier has not accepted this job yet");
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
        self.settle(job, outcome, Some(evidence), false);
    }

    /// Anyone may close a job that the verifier never ruled on, after deadline + grace.
    /// An active job settles as FAIL for the market (there is no verified delivery, so
    /// cover pays), but it is the verifier's fault: its fee goes back to the buyer, and
    /// its pledge goes to the agent (up to the agent's lost bond) and then to the buyer.
    /// The agent's record counts it as unverified, not as a fail.
    /// A posted job nobody accepted is cancelled and the buyer refunded.
    pub fn expire(&mut self, job_id: u64) {
        let mut job = self.job_mut(job_id);
        require!(env::block_timestamp() >= job.deadline_ns + job.grace_ns, "too early to expire");
        match job.status {
            JobStatus::Active => self.settle(job, Outcome::Fail, Some("expired: the verifier never ruled".into()), true),
            JobStatus::Posted => {
                job.status = JobStatus::Cancelled;
                let _ = Promise::new(job.buyer.clone()).transfer(yocto(job.payment.0 + job.verifier_fee.0));
                if job.verifier_accepted {
                    let mut v = self.verifier_of(&job.verifier);
                    v.locked = U128(v.locked.0 - job.verifier_pledge.0);
                    self.verifiers.insert(job.verifier.clone(), v);
                }
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
    // Agent marketplace
    // ------------------------------------------------------------------

    /// List (or update) this account's agent profile in the marketplace.
    /// The first listing needs a 0.05 NEAR deposit for storage.
    #[payable]
    pub fn list_agent(&mut self, name: String, specialties: Vec<String>, models: Vec<String>, description: String, endpoint: String, rate: U128) {
        let who = env::predecessor_account_id();
        require!(!name.is_empty() && name.len() <= 64, "name: 1 to 64 characters");
        require!(description.len() <= 400 && endpoint.len() <= 200, "description up to 400, endpoint up to 200 characters");
        check_tags(&specialties, "specialties");
        check_tags(&models, "models");
        let first = !self.profiles.contains_key(&who);
        if first {
            require!(env::attached_deposit().as_yoctonear() >= LISTING_DEPOSIT, "listing needs a 0.05 NEAR deposit");
            self.profile_list.push(who.clone());
        }
        self.profiles.insert(who.clone(), AgentProfile { name, specialties, models, description, endpoint, rate, listed_ns: env::block_timestamp() });
        env::log_str(&format!("EVENT_JSON:{{\"event\":\"agent_listed\",\"agent\":\"{who}\"}}"));
    }

    pub fn get_agent_listing(&self, agent: AccountId) -> Option<AgentListing> {
        self.profiles.get(&agent).cloned().map(|profile| self.listing(agent, profile))
    }

    pub fn list_agent_listings(&self, from: Option<u64>, limit: Option<u64>) -> Vec<AgentListing> {
        let from = from.unwrap_or(0);
        let limit = limit.unwrap_or(50).min(100);
        (from..(from + limit).min(self.profile_list.len() as u64))
            .filter_map(|i| self.profile_list.get(i as u32).cloned())
            .filter_map(|a| self.profiles.get(&a).cloned().map(|p| self.listing(a, p)))
            .collect()
    }

    // ------------------------------------------------------------------
    // Verifiers
    // ------------------------------------------------------------------

    /// Apply (or re-apply) to be a verifier: attach a bond, name your Hive account and fee.
    /// The Hive oracle then checks the account's reputation and approves or rejects.
    /// Changing the Hive account sends the application back to Pending.
    #[payable]
    pub fn apply_verifier(&mut self, hive_account: String, description: String, specialties: Vec<String>, fee_bps: u32) -> VerifierStatus {
        let who = env::predecessor_account_id();
        let deposit = env::attached_deposit().as_yoctonear();
        require!(is_hive_name(&hive_account), "not a valid Hive account name");
        require!(description.len() <= 400, "description up to 400 characters");
        check_tags(&specialties, "specialties");
        require!(fee_bps <= MAX_VERIFIER_FEE_BPS, "verifier fee is capped at 20%");
        let now = env::block_timestamp();
        let mut v = match self.verifiers.get(&who).cloned() {
            Some(v) => v,
            None => {
                self.verifier_list.push(who.clone());
                Verifier {
                    account: who.clone(), hive_account: String::new(), hive_reputation: 0, status: VerifierStatus::Pending,
                    description: String::new(), specialties: vec![], fee_bps: 0, bond: U128(0), locked: U128(0),
                    accepted: 0, declined: 0, passes: 0, fails: 0, timeouts: 0, earned: U128(0), slashed: U128(0), applied_ns: now, attested_ns: 0,
                }
            }
        };
        if v.hive_account != hive_account || v.status == VerifierStatus::Retired {
            v.status = VerifierStatus::Pending;
            v.hive_reputation = 0;
            v.applied_ns = now;
        }
        v.hive_account = hive_account;
        v.description = description;
        v.specialties = specialties;
        v.fee_bps = fee_bps;
        v.bond = U128(v.bond.0 + deposit);
        require!(v.bond.0 >= self.min_verifier_bond, "verifier bond must be at least 1 NEAR");
        let status = v.status;
        self.verifiers.insert(who.clone(), v);
        env::log_str(&format!("EVENT_JSON:{{\"event\":\"verifier_applied\",\"verifier\":\"{who}\"}}"));
        status
    }

    /// The Hive oracle reports a verifier's Hive reputation (score x 100), after checking
    /// that the applicant signed `actuary:verifier:<near account>` with that Hive account's key.
    pub fn attest_hive(&mut self, verifier: AccountId, hive_account: String, reputation_x100: u32) -> VerifierStatus {
        require!(env::predecessor_account_id() == self.hive_oracle, "only the Hive oracle can attest");
        let mut v = self.verifier_of(&verifier);
        require!(v.hive_account == hive_account, "Hive account does not match the application");
        require!(v.status != VerifierStatus::Retired, "verifier has retired");
        v.hive_reputation = reputation_x100;
        v.attested_ns = env::block_timestamp();
        v.status = if reputation_x100 >= self.min_hive_rep { VerifierStatus::Approved } else { VerifierStatus::Rejected };
        let status = v.status;
        self.verifiers.insert(verifier.clone(), v);
        env::log_str(&format!("EVENT_JSON:{{\"event\":\"hive_attested\",\"verifier\":\"{verifier}\",\"reputation_x100\":{reputation_x100},\"status\":\"{status:?}\"}}"));
        status
    }

    /// Withdraw verifier bond that is not pledged to open jobs. The rest must stay at least
    /// the minimum bond, or be zero (the verifier retires).
    pub fn withdraw_verifier_bond(&mut self, amount: U128) -> Promise {
        let who = env::predecessor_account_id();
        let mut v = self.verifier_of(&who);
        require!(amount.0 > 0 && amount.0 <= v.bond.0 - v.locked.0, "that much bond is not free");
        let rest = v.bond.0 - amount.0;
        require!(rest == 0 || rest >= self.min_verifier_bond, "leave at least 1 NEAR or withdraw everything");
        v.bond = U128(rest);
        if rest == 0 {
            v.status = VerifierStatus::Retired;
        }
        self.verifiers.insert(who.clone(), v);
        Promise::new(who).transfer(yocto(amount.0))
    }

    pub fn set_hive_oracle(&mut self, oracle: AccountId) {
        require!(env::predecessor_account_id() == self.owner, "only the owner");
        self.hive_oracle = oracle;
    }

    pub fn get_verifier(&self, verifier: AccountId) -> Option<Verifier> {
        self.verifiers.get(&verifier).cloned()
    }

    pub fn list_verifiers(&self, from: Option<u64>, limit: Option<u64>) -> Vec<Verifier> {
        let from = from.unwrap_or(0);
        let limit = limit.unwrap_or(50).min(100);
        (from..(from + limit).min(self.verifier_list.len() as u64))
            .filter_map(|i| self.verifier_list.get(i as u32).cloned())
            .filter_map(|a| self.verifiers.get(&a).cloned())
            .collect()
    }

    /// What a buyer attaches to `post_job` for this verifier and payment: (payment, verifier fee, total).
    pub fn job_cost(&self, verifier: AccountId, payment: U128) -> (U128, U128, U128) {
        let v = self.verifier_of(&verifier);
        let fee = payment.0 * v.fee_bps as u128 / BPS;
        (payment, U128(fee), U128(payment.0 + fee))
    }

    /// (hive_oracle, min_hive_rep_x100, min_verifier_bond, verifier_pledge_bps)
    pub fn get_verifier_config(&self) -> (AccountId, u32, U128, u32) {
        (self.hive_oracle.clone(), self.min_hive_rep, U128(self.min_verifier_bond), self.verifier_pledge_bps)
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

    fn verifier_of(&self, who: &AccountId) -> Verifier {
        self.verifiers.get(who).cloned().unwrap_or_else(|| env::panic_str("verifier is not registered"))
    }

    fn listing(&self, account: AccountId, profile: AgentProfile) -> AgentListing {
        let record = self.agents.get(&account).cloned().unwrap_or_default();
        let prior_bps = prior_from_record(record.passes, record.fails);
        AgentListing { account, profile, record, prior_bps }
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

    fn settle(&mut self, mut job: Job, outcome: Outcome, evidence: Option<String>, timed_out: bool) {
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

        // Verifier: paid its fee for ruling, whatever the verdict. If it never ruled, the fee
        // goes back to the buyer and its pledge compensates the agent's lost bond, then the buyer.
        let mut v = self.verifier_of(&job.verifier);
        v.locked = U128(v.locked.0 - job.verifier_pledge.0);
        if timed_out {
            job.verifier_timed_out = true;
            let pledge = job.verifier_pledge.0;
            v.bond = U128(v.bond.0 - pledge);
            v.slashed = U128(v.slashed.0 + pledge);
            v.timeouts += 1;
            let to_agent = pledge.min(job.bond.0);
            if to_agent > 0 {
                let _ = Promise::new(job.agent.clone()).transfer(yocto(to_agent));
            }
            let to_buyer = job.verifier_fee.0 + pledge - to_agent;
            if to_buyer > 0 {
                let _ = Promise::new(job.buyer.clone()).transfer(yocto(to_buyer));
            }
        } else {
            if outcome == Outcome::Pass { v.passes += 1 } else { v.fails += 1 }
            v.earned = U128(v.earned.0 + job.verifier_fee.0);
            if job.verifier_fee.0 > 0 {
                let _ = Promise::new(job.verifier.clone()).transfer(yocto(job.verifier_fee.0));
            }
        }
        self.verifiers.insert(job.verifier.clone(), v);

        // Agent record (reputation).
        let mut rec = self.agents.get(&job.agent).cloned().unwrap_or_default();
        if timed_out {
            rec.unverified += 1;
            self.agents.insert(job.agent.clone(), rec);
            let id = job.id;
            env::log_str(&format!("EVENT_JSON:{{\"event\":\"job_unverified\",\"job\":{id}}}"));
            self.jobs.replace(id as u32, job);
            return;
        }
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

fn check_tags(tags: &[String], what: &str) {
    require!(tags.len() <= 8, format!("{what}: at most 8"));
    require!(tags.iter().all(|t| !t.is_empty() && t.len() <= 40), format!("{what}: each 1 to 40 characters"));
}

/// Hive account names: 3 to 16 characters, lowercase letters, digits, dots and dashes.
fn is_hive_name(s: &str) -> bool {
    (3..=16).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-') && s.as_bytes()[0].is_ascii_lowercase()
}

#[cfg(test)]
mod tests;
