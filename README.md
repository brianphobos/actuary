# Actuary

**The insurance market for AI agent work, on NEAR.**

Every job an AI agent takes gets its own two-outcome market: **PASS** (the work is delivered and verified) or **FAIL**. The PASS price is a live, money-backed estimate of whether the agent will deliver. One market gives you three things that today are built (badly) as three separate products:

| You want | In Actuary it is | How |
|---|---|---|
| Insurance for a buyer | Holding FAIL shares | Each FAIL share pays 1 NEAR-unit if the job fails. The price you pay is the premium. |
| A performance bond from the agent | The agent's PASS stake | An agent must buy PASS to accept a job, and can never buy FAIL on its own work. |
| Reputation for agents | The agent's on-chain record | Each job opens at a price set by the agent's record. Every resolved job updates it. |

Liquidity comes from an **underwriting vault**: LPs deposit once, the contract seeds every new job market from it, and the vault earns trading fees plus a 2% take on delivered work. A **verifier** (designed to run in a TEE) attests progress heartbeats and the final result. Heartbeats move the price automatically.

> Escrow gets your money back. Actuary makes you whole.

Built for the NEAR HACKATHON, Nov 2026. Tracks: **Agent Economy**, **AI Security & Defense**, **Unstoppable DeFi**, **AI Swarms**, **Open Innovation**.

---

## Run it in 2 minutes (dry run: no wallet, no network, no funds)

Requirements: Node.js 18+ (no npm packages needed). Rust 1.80+ only for the contract tests.

1. Check the JavaScript engine matches the contract exactly:
   ```bash
   node sim/test.js
   ```
   You should see `engine.js matches contract vectors: OK (19 checks)` and `heartbeat repricing conserves money: OK`.

2. Walk through the demo story (13 steps, about 1 second):
   ```bash
   node sim/run.js --story
   ```
   You should see a rookie agent (nova) win a job by bonding, a buyer buy cover, attested heartbeats lift PASS to 97%, a TEE-style verifier pass the work, then a trusted agent (sleeper, 12 straight passes) try to bet against itself (refused by the contract), miss its milestones, fall to 3%, and fail. The buyer's cover pays out 15 NEAR.

3. Run the swarm market for 600 simulated hours:
   ```bash
   node sim/run.js --ticks 600 --seed 7
   ```
   With seed 7 you should see about 320 resolved jobs; a Brier score of **0.042** at market close versus **0.185** from track record alone (the market knows far more than the résumé); the vault ending slightly **up** (+0.66%) after paying claims; both rookies hired within 7 hours of joining; and the reputation-farming `sleeper.agent` caught with closing prices of 3–19% on the jobs it betrayed. The full report is written to `sim/out/report.json`.

4. Run the reference verifier on a good and a bad delivery:
   ```bash
   node verifier/verify.js verifier/examples/slugify verifier/examples/slugify/good.js   # PASS 24/24, exit 0
   node verifier/verify.js verifier/examples/slugify verifier/examples/slugify/bad.js    # FAIL 16/24, exit 1
   ```
   Each prints an evidence hash and the exact `near` command a verifier account would sign to resolve the job.

5. Run the contract's unit tests:
   ```bash
   cd contract && cargo test
   ```
   You should see `test result: ok. 15 passed`.

   Optional, end to end on a real local NEAR node (nearcore sandbox) with the compiled wasm:
   ```bash
   cargo install cargo-near            # once
   cargo near build non-reproducible-wasm
   cargo test --test sandbox -- --ignored --nocapture
   ```
   This deploys `target/near/actuary.wasm`, runs both story jobs with real transactions (bond, cover, heartbeats, verified PASS with the agent paid 9.8 NEAR, the refused self-short, FAIL with the buyer refunded plus a cover payout) and prints `test result: ok. 1 passed`.

6. Open the dashboard: open `dashboard/dist/actuary.html` in a browser (it is a single self-contained file). The **Story** tab steps through the demo with a live price chart, ledger and on-chain records. The **Swarm market** tab runs the swarm live: workers, buyers and the underwriter swarm on one floor, with live job arcs colored by PASS odds, the vault, calibration and a tape. To rebuild it after changes: `node dashboard/build.js`.

`npm test` runs steps 1, 4 and 5 together.

## Run it on NEAR testnet

```bash
cargo install cargo-near          # once
PARENT=yourname.testnet ./scripts/testnet-demo.sh
```

The script builds the contract, creates seven sub-accounts under `PARENT`, deploys, and replays the story with real transactions: vault deposit, job, bond, cover, heartbeats, verifier resolution with the evidence hash from `verifier/verify.js`, the refused self-short, missed heartbeats, `expire` after the grace period, and redemption. It spends about 6.5 testnet NEAR (the faucet gives 10) and takes about 5 minutes (it waits for a 3-minute deadline). It prints a nearblocks link at the end.

The script has been run end to end with near-cli-rs 0.31.1 against a local nearcore 2.13.4 node (`NETWORK=sandbox PARENT=test.near`, with a `sandbox` connection added to `~/.config/near-cli/config.toml`). The only differences on testnet are the network and how you fund `PARENT`.

## How it works

```
buyer ──post_job(payment)──▶ escrow
agent ──accept_job(bond)───▶ vault seeds pool at the agent's reputation price, bond buys PASS
buyer ──buy(Fail)──────────▶ cover (FAIL shares); the price paid is the premium
swarm ──buy(Pass|Fail)─────▶ price discovery
verifier ─heartbeat(on_time)▶ market maker reprices itself (Bayes update)
verifier ─resolve(outcome)─▶ PASS: agent paid (minus 2% take), PASS holders redeem
                             FAIL: buyer refunded, FAIL holders (cover) redeem, bond lost
anyone ──expire()──────────▶ no verdict by deadline + grace ⇒ FAIL
```

**Market maker.** A Fixed Product Market Maker (the Gnosis/Polymarket FPMM family), so all math is integer multiply and divide (`contract/src/math.rs`, 256-bit intermediates). One collateral unit mints one PASS and one FAIL share; winning shares redeem 1:1, so every job is fully collateralized.

**Opening price.** `(passes + 1) / (passes + fails + 2)`, clamped to 5–95%. A new agent opens at 50%. The agent's bond then buys PASS, which moves the price up: a rookie can buy its way to credibility with its own money, and that money pays insured buyers if it fails.

**Attested heartbeats.** The verifier posts milestone results (`heartbeat(job_id, on_time, evidence)`). The contract applies a Bayes update with `P(on time | will pass) = 92%` and `P(on time | will fail) = 30%`, and re-centres the pool using only outcome tokens the vault already owns. This is the key design decision, and the simulator found it (see below).

**Rules the contract enforces:** an agent cannot buy FAIL on its own job; the verifier cannot trade; only the verifier resolves or attests; buyers cannot hire themselves; the agent cannot verify itself; the bond must be at least 10% of the payment; LPs can only withdraw capital that is not locked in open jobs; markets close at the deadline; anyone can expire a job after deadline + grace.

### Contract API (`contract/src/lib.rs`)

| Method | Who | Deposit | Does |
|---|---|---|---|
| `new(owner, fee_bps?, liquidity_bps?, min_bond_bps?, take_bps?)` | deployer | – | defaults: 3% trading fee, liquidity = 100% of payment, bond ≥ 10%, take 2% |
| `vault_deposit()` | LP | NEAR | mint vault shares |
| `vault_withdraw(shares)` | LP | – | burn shares for free capital |
| `post_job(agent, verifier, spec, deadline_ns, grace_ns?)` | buyer | payment | escrow, price from agent record |
| `accept_job(job_id)` | agent | bond | seed market, bond buys PASS |
| `buy(job_id, outcome, min_shares?)` | anyone | amount | FPMM buy with slippage guard |
| `heartbeat(job_id, on_time, evidence)` | verifier | – | Bayes reprice |
| `resolve(job_id, outcome, evidence)` | verifier | – | settle escrow, vault, record |
| `expire(job_id)` | anyone | – | FAIL after deadline + grace, or cancel an unaccepted job |
| `redeem(job_id)` | holder | – | pay winning shares |
| views | – | – | `get_job`, `list_jobs`, `pass_price_bps`, `quote`, `get_position`, `get_agent`, `agent_prior_bps`, `get_vault`, `get_vault_shares`, `get_config` |

Every state change logs a NEP-297-style `EVENT_JSON:` line (`job_posted`, `job_accepted`, `trade`, `heartbeat`, `job_resolved`) for indexers.

## What the simulator taught us

The swarm (`sim/swarm.js`) runs scripted agents on `sim/engine.js`, a BigInt mirror of the contract that is checked against the Rust code with shared test vectors. No LLM calls, seeded randomness.

The first version had no heartbeats in the contract: watcher agents read progress signals off-chain and traded on them. The market became accurate, but **the vault lost money on every seed**: −17% over 600 simulated hours at the final fee settings, and −48% with deeper liquidity. Every public progress signal was a free option for whoever traded first against the passive pool, and raising fees did not fix it.

The fix was to make progress an on-chain, attested input that the market maker uses directly. Averaged over seeds 1, 2, 3 and 7 with the same fees, the vault went from **−17.3%** to **+1.0%**, and the Brier score at close improved from 0.071 to 0.045. Traders still profit from information the verifier does not see (the watchers in the sim read commits with 70% accuracy), which is the reason to have a market at all.

Reproduce the comparison: `node sim/run.js --ticks 600 --seed <n>` for n in 1 2 3 7.

## Threat model and limits

- **The verifier is trusted.** It resolves jobs and attests heartbeats. The design target is a TEE (OutLayer or NEAR AI Cloud) running a job's acceptance suite, so the evidence hash carries a hardware attestation; `verifier/verify.js` is the reference logic and runs locally in this build. It `require`s the delivered code, so outside a TEE run it only in a sandbox.
- **Self-shorting through a second account.** The contract stops an agent buying FAIL on its own job from its own account, not from a sybil. An agent profits from sabotage only if its FAIL position pays more than its lost bond and lost payment; FAIL buys move the price, so this shows up on-chain. A per-job cap on FAIL open interest relative to the bond is the next step.
- **Heartbeat likelihoods are fixed** (92% / 30%). They should be learned per verifier from resolved jobs.
- **No sell.** Positions are held to resolution. FPMM sell needs a quadratic solve; not built yet.
- **NEAR only.** Collateral is native NEAR so the testnet demo needs nothing else. USDC (NEP-141 `ft_on_transfer`) and paying cover out on any chain through NEAR Intents are the next integrations.
- **Simple payouts.** Transfers have no callbacks. A payout to an account that does not exist returns to the contract.
- **Not audited.** Testnet only.

## Repository

```
contract/        NEAR smart contract (Rust, near-sdk 5), 15 unit tests
  tests/sandbox.rs end-to-end test on a local NEAR node
  src/lib.rs     jobs, vault, escrow, market, records
  src/math.rs    FPMM, seeding, Bayes repricing (integer math)
  src/tests.rs   unit tests, shared vectors with sim/test.js
sim/
  engine.js      BigInt mirror of the contract (Node and browser)
  swarm.js       scripted swarm + story mode
  run.js         dry-run CLI
  test.js        cross-checks engine.js against the Rust vectors
verifier/        reference verifier and the slugify example job
dashboard/       index.html + build.js → dist/actuary.html (single file)
scripts/         testnet-demo.sh
docs/            submission text, demo video script, X post
```

## Built with

- near-sdk 5 (Rust), the `uint` crate for 256-bit math. No other dependencies.
- Plain JavaScript (no npm packages). Google Fonts in the dashboard.
- The FPMM design follows Gnosis's conditional-token market maker.
- AI disclosure: this project was written with Claude (Anthropic) as a coding agent, per the hackathon rules on AI-generated code.

All code was written during the hackathon window. MIT licensed (see `LICENSE`).
