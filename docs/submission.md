# Submission draft (for Brian to fill in and send)

Form: https://nearhackathon.com/#apply · deadline **14 Nov 2026, 23:59 UTC**. Nothing has been submitted.

| Field | Value |
|---|---|
| Project name | Actuary |
| Team lead | *(your name)* |
| Email | *(your email; max 2 submissions per email)* |
| Team size | 1 (Solo) |
| Tracks | Agent Economy (`agents`), AI Security & Defense (`security`), Unstoppable DeFi (`defi`), AI Swarms (`swarms`), Open Innovation (`open`) |
| NEAR wallet (mainnet, receives prize) | *(your .near or implicit account, lowercase)* |
| Repository link | *(public GitHub URL after you push)* |
| Demo post link | *(your X post URL; see x-post.md)* |
| What did you build? | paste `docs/idea.txt` (1,7xx characters, under the 2,000 limit) |
| Website | optional: the dashboard link, if you host `dashboard/dist/actuary.html` (e.g. GitHub Pages) |

## Before you submit

1. Push the repo to a new **public** GitHub repo (it must stay public through judging).
2. Run `PARENT=yourname.testnet ./scripts/testnet-demo.sh` once and paste the nearblocks contract link into the README under "Run it on NEAR testnet". The script already passes end to end against a local NEAR node with the real CLI; it just has not touched public testnet yet, because the build sandbox could not reach testnet RPC. Needs `cargo install cargo-near near-cli-rs` and Node 18+.
3. Record the video (see `video-script.md`) and post on X (see `x-post.md`).
4. Fill in the form. Do not resubmit to change things later; use https://nearhackathon.com/request-changes.

## Track fit, one line each

- **Agent Economy:** agents earn, stake, and hire with priced risk; rookies get hired by bonding.
- **AI Security & Defense:** reputation, bonds, a refused self-short, a verifier, and a reputation-farming attack caught by the market.
- **Unstoppable DeFi:** a new on-chain primitive (per-job FPMM with an underwriting vault and oracle repricing), no custodian.
- **AI Swarms:** the underwriter swarm (actuaries, watchers, noise traders) is how prices form; 20+ scripted agents in the sim.
- **Open Innovation:** insurance for agent work did not exist among the other 618 submissions we checked.
