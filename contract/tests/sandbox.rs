//! End-to-end test on a local NEAR node (near-sandbox) with the real wasm.
//! Build first: `cargo near build non-reproducible-wasm`, then
//! `cargo test --test sandbox -- --ignored --nocapture` (downloads near-sandbox on first run).
use near_workspaces::types::NearToken;
use serde_json::json;

const WASM: &str = "target/near/actuary.wasm";

#[tokio::test]
#[ignore = "needs the built wasm and a near-sandbox download"]
async fn story_on_a_real_node() -> anyhow::Result<()> {
    let wasm = std::fs::read(WASM).expect("run `cargo near build non-reproducible-wasm` first");
    let worker = near_workspaces::sandbox().await?;
    let contract = worker.dev_deploy(&wasm).await?;
    let root = worker.root_account()?;
    let mk = |name: &'static str| {
        let root = root.clone();
        async move { root.create_subaccount(name).initial_balance(NearToken::from_near(50)).transact().await.map(|r| r.into_result().unwrap()) }
    };
    let (lp, buyer, nova, verifier, globex, sleeper) = (mk("lp").await?, mk("acme").await?, mk("nova").await?, mk("verifier").await?, mk("globex").await?, mk("sleeper").await?);
    let oracle = mk("hive-oracle").await?;

    contract.call("new").args_json(json!({"owner": root.id(), "hive_oracle": oracle.id()})).transact().await?.into_result()?;
    // nova lists itself in the agent marketplace.
    nova.call(contract.id(), "list_agent")
        .args_json(json!({"name": "Nova", "specialties": ["javascript", "string utils"], "models": ["claude-opus-5-5"], "description": "Small JS utilities with tests.", "endpoint": "https://nova.example/jobs", "rate": NearToken::from_near(10).as_yoctonear().to_string()}))
        .deposit(NearToken::from_millinear(50)).transact().await?.into_result()?;
    // The verifier applies with a 10 NEAR bond and a 5% fee; the Hive oracle attests reputation 61.20.
    verifier.call(contract.id(), "apply_verifier")
        .args_json(json!({"hive_account": "vera", "description": "Runs JS test suites in a TEE.", "specialties": ["javascript"], "fee_bps": 500}))
        .deposit(NearToken::from_near(10)).transact().await?.into_result()?;
    let early = buyer.call(contract.id(), "post_job")
        .args_json(json!({"agent": nova.id(), "verifier": verifier.id(), "spec": "x", "payment": "1000000000000000000000000", "deadline_ns": u64::MAX / 2}))
        .deposit(NearToken::from_millinear(1050)).transact().await?;
    assert!(format!("{:?}", early.into_result().unwrap_err()).contains("verifier is not approved"));
    oracle.call(contract.id(), "attest_hive").args_json(json!({"verifier": verifier.id(), "hive_account": "vera", "reputation_x100": 6120})).transact().await?.into_result()?;
    lp.call(contract.id(), "vault_deposit").deposit(NearToken::from_near(30)).transact().await?.into_result()?;

    // Job 0: rookie nova, bonded, insured, heartbeats, verified PASS.
    let deadline = u64::MAX / 2;
    let id: u64 = buyer.call(contract.id(), "post_job")
        .args_json(json!({"agent": nova.id(), "verifier": verifier.id(), "spec": "sha256:slugify", "payment": NearToken::from_near(10).as_yoctonear().to_string(), "deadline_ns": deadline}))
        .deposit(NearToken::from_millinear(10_500)).transact().await?.json()?;
    assert_eq!(id, 0);
    verifier.call(contract.id(), "accept_verification").args_json(json!({"job_id": 0})).transact().await?.into_result()?;
    let p: u32 = contract.view("pass_price_bps").args_json(json!({"job_id": 0})).await?.json()?;
    assert_eq!(p, 5000);
    nova.call(contract.id(), "accept_job").args_json(json!({"job_id": 0})).deposit(NearToken::from_near(4)).transact().await?.into_result()?;
    buyer.call(contract.id(), "buy").args_json(json!({"job_id": 0, "outcome": "Fail"})).deposit(NearToken::from_near(2)).max_gas().transact().await?.into_result()?;
    let before: u32 = contract.view("pass_price_bps").args_json(json!({"job_id": 0})).await?.json()?;
    let after: u32 = verifier.call(contract.id(), "heartbeat").args_json(json!({"job_id": 0, "on_time": true, "evidence": "sha256:c1"})).transact().await?.json()?;
    assert!(after > before);
    let nova_before = nova.view_account().await?.balance;
    verifier.call(contract.id(), "resolve").args_json(json!({"job_id": 0, "outcome": "Pass", "evidence": "tests 24/24"})).transact().await?.into_result()?;
    let nova_after = nova.view_account().await?.balance;
    assert_eq!(nova_after.as_yoctonear() - nova_before.as_yoctonear(), NearToken::from_millinear(9800).as_yoctonear(), "agent paid 10 NEAR minus 2% take");
    let paid: String = nova.call(contract.id(), "redeem").args_json(json!({"job_id": 0})).transact().await?.json::<String>()?;
    assert!(paid.parse::<u128>()? > NearToken::from_near(4).as_yoctonear(), "bond comes back with profit");

    // Job 1: sleeper tries to short itself (refused), misses heartbeats, verified FAIL.
    globex.call(contract.id(), "post_job")
        .args_json(json!({"agent": sleeper.id(), "verifier": verifier.id(), "spec": "sha256:migration", "payment": NearToken::from_near(10).as_yoctonear().to_string(), "deadline_ns": deadline}))
        .deposit(NearToken::from_millinear(10_500)).transact().await?.into_result()?;
    verifier.call(contract.id(), "accept_verification").args_json(json!({"job_id": 1})).transact().await?.into_result()?;
    sleeper.call(contract.id(), "accept_job").args_json(json!({"job_id": 1})).deposit(NearToken::from_near(1)).transact().await?.into_result()?;
    globex.call(contract.id(), "buy").args_json(json!({"job_id": 1, "outcome": "Fail"})).deposit(NearToken::from_near(2)).transact().await?.into_result()?;
    let refused = sleeper.call(contract.id(), "buy").args_json(json!({"job_id": 1, "outcome": "Fail"})).deposit(NearToken::from_near(1)).transact().await?;
    assert!(format!("{:?}", refused.into_result().unwrap_err()).contains("an agent cannot bet against its own work"));
    for _ in 0..2 {
        verifier.call(contract.id(), "heartbeat").args_json(json!({"job_id": 1, "on_time": false, "evidence": "missed"})).transact().await?.into_result()?;
    }
    let p: u32 = contract.view("pass_price_bps").args_json(json!({"job_id": 1})).await?.json()?;
    assert!(p < 1500, "price collapses after missed heartbeats, got {p}");
    let g0 = globex.view_account().await?.balance.as_yoctonear();
    verifier.call(contract.id(), "resolve").args_json(json!({"job_id": 1, "outcome": "Fail", "evidence": "no delivery"})).transact().await?.into_result()?;
    let cover: String = globex.call(contract.id(), "redeem").args_json(json!({"job_id": 1})).transact().await?.json()?;
    let g1 = globex.view_account().await?.balance.as_yoctonear();
    // escrow refund (10) + cover payout, minus a little gas
    assert!(g1 - g0 > NearToken::from_near(10).as_yoctonear() + cover.parse::<u128>()? - NearToken::from_millinear(10).as_yoctonear());

    let rec: serde_json::Value = contract.view("get_agent").args_json(json!({"agent": sleeper.id()})).await?.json()?;
    assert_eq!(rec["fails"], 1);
    let v: serde_json::Value = contract.view("get_verifier").args_json(json!({"verifier": verifier.id()})).await?.json()?;
    assert_eq!((v["passes"].as_u64(), v["fails"].as_u64(), v["earned"].as_str(), v["locked"].as_str()), (Some(1), Some(1), Some("1000000000000000000000000"), Some("0")), "verifier paid 0.5 NEAR per ruling, pledges released");
    let l: serde_json::Value = contract.view("list_agent_listings").args_json(json!({})).await?.json()?;
    assert_eq!(l[0]["record"]["passes"], 1);
    println!("marketplace: {}", l[0]);
    let vault: serde_json::Value = contract.view("get_vault").args_json(json!({})).await?.json()?;
    println!("vault after two jobs: {vault}");
    println!("cover paid to globex: {cover}");
    Ok(())
}
