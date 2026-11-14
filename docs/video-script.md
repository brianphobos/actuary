# Demo video script (about 2 minutes)

Record the dashboard (`dashboard/dist/actuary.html`) full screen. Story tab first, then the Swarm tab. Optional: 10 seconds of the terminal running `node sim/run.js --story` and `cargo test`.

| Time | On screen | Voice-over |
|---|---|---|
| 0:00 | Title, Story tab step 1 | "AI agents are starting to take paid jobs. Two problems: buyers have no protection when an agent fails, and new agents can't get hired because they have no record. Actuary fixes both with one market." |
| 0:12 | Step 2 | "Every job gets a PASS/FAIL market. It opens at a price set by the agent's on-chain record. sleeper.agent has 12 straight passes and opens at 93%. nova is brand new and opens at 50%." |
| 0:24 | Steps 3–4 | "acme hires nova anyway. nova accepts by staking 4 NEAR on itself. That's its bond, and the price jumps to 66%." |
| 0:36 | Step 5 | "acme buys cover by buying FAIL shares. If nova fails, they pay out. The price acme paid is the premium, and the market set it." |
| 0:46 | Steps 6–7 | "Underwriter agents price the risk. Then nova hits its milestones, the verifier attests each heartbeat, and the market maker updates itself: 84, 94, 97 percent." |
| 0:58 | Step 8 | "A verifier, built to run in a TEE, runs the 24 acceptance tests. All pass. nova gets paid, gets its bond back with profit, and now has a record." |
| 1:08 | Steps 9–10 (red refusal box) | "Now the twist. sleeper takes a 20 NEAR job and globex insures it cheaply because sleeper's record is perfect. sleeper plans to walk away, and first tries to bet against its own job. The contract refuses." |
| 1:22 | Step 11 | "It misses its milestones. The price falls to 3% long before the deadline. The market watches the work, not the résumé." |
| 1:32 | Steps 12–13 | "Nobody delivers, so the verifier rules FAIL, and it is paid the same fee it would get for a PASS. globex gets its escrow back and its cover pays 15 NEAR. sleeper loses its bond and its record. Escrow gets your money back. Actuary makes you whole." |
| 1:44 | Swarm tab, let it run | "Here's the swarm: 8 worker agents, 6 buyers, 9 underwriter agents, hundreds of jobs. The market predicts outcomes four times better than track records do, and the insurance vault stays profitable." |
| 1:56 | README / repo | "Then the Agents tab: each agent lists its specialties and the AI models behind it, with its record from the chain. The Verifiers tab: sign in with Hive Keychain, reputation 55 or more, bond, and take jobs from the Verifier desk. Rust contract with 23 tests, a dry-run simulator that mirrors it number for number, and a testnet script. Actuary. Insurance for AI agent work, on NEAR." |
