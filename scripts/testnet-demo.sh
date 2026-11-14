#!/usr/bin/env bash
# Runs the Actuary story on NEAR testnet with real transactions.
#
#   PARENT=yourname.testnet ./scripts/testnet-demo.sh
#
# Needs: near-cli-rs (`near`), cargo-near, and a funded testnet account PARENT
# whose key is in the legacy keychain (~/.near-credentials). Get one with:
#   near account create-account sponsor-by-faucet-service yourname.testnet autogenerate-new-keypair save-to-legacy-keychain network-config testnet create
# The script spends about 8.7 testnet NEAR on sub-accounts. No mainnet funds.
set -euo pipefail

PARENT="${PARENT:?set PARENT to your funded testnet account, e.g. PARENT=alice.testnet}"
NETWORK="${NETWORK:-testnet}"
NET="network-config $NETWORK"
TAG="${TAG:-a$(date +%s | tail -c 6)}"
C="actuary-$TAG.$PARENT"
LP="lp-$TAG.$PARENT"; BUYER="acme-$TAG.$PARENT"; GLOBEX="globex-$TAG.$PARENT"
NOVA="nova-$TAG.$PARENT"; SLEEPER="sleeper-$TAG.$PARENT"; VERIFIER="verifier-$TAG.$PARENT"; ORACLE="hive-oracle-$TAG.$PARENT"
YOCTO_HALF="500000000000000000000000" # 0.5 NEAR
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WASM="$ROOT/contract/target/near/actuary.wasm"

say() { printf '\n\033[1;33m== %s\033[0m\n' "$*"; }
call() { # call <signer> <method> <json> <deposit NEAR>
  near contract call-function as-transaction "$C" "$2" json-args "$3" prepaid-gas '30 Tgas' attached-deposit "$4 NEAR" sign-as "$1" $NET sign-with-legacy-keychain send
}
view() { near contract call-function as-read-only "$C" "$1" json-args "$2" $NET now; }
mk() { near account create-account fund-myself "$1" "$2 NEAR" autogenerate-new-keypair save-to-legacy-keychain sign-as "$PARENT" $NET sign-with-legacy-keychain send; }
now_ns() { echo $(( $(date +%s) * 1000000000 )); }

say "Build the contract"
(cd "$ROOT/contract" && cargo near build non-reproducible-wasm)

say "Create accounts under $PARENT"
mk "$C" 3.5; mk "$LP" 1.2; mk "$BUYER" 0.8; mk "$GLOBEX" 0.9; mk "$NOVA" 0.5; mk "$SLEEPER" 0.3; mk "$VERIFIER" 1.4; mk "$ORACLE" 0.1

say "Deploy $C"
near contract deploy "$C" use-file "$WASM" with-init-call new json-args "{\"owner\":\"$PARENT\",\"hive_oracle\":\"$ORACLE\"}" prepaid-gas '100 Tgas' attached-deposit '0 NEAR' $NET sign-with-legacy-keychain send

say "1. LP funds the underwriting vault with 1 NEAR"
call "$LP" vault_deposit '{}' 1

say "1b. nova lists itself in the agent marketplace"
call "$NOVA" list_agent '{"name":"Nova","specialties":["javascript","string utils"],"models":["claude-opus-5-5"],"description":"Small JS utilities, shipped with tests.","endpoint":"https://nova.example/jobs","rate":"500000000000000000000000"}' 0.05

say "1c. A verifier applies with a 1 NEAR bond, Hive account and 5% fee; the Hive oracle approves it"
call "$VERIFIER" apply_verifier '{"hive_account":"vera","description":"Runs acceptance suites; evidence hash on chain.","specialties":["javascript"],"fee_bps":500}' 1
# In production the oracle first runs: node verifier/hive-oracle.js <verifier> <hive account> <keychain signature>
call "$ORACLE" attest_hive "{\"verifier\":\"$VERIFIER\",\"hive_account\":\"vera\",\"reputation_x100\":6120}" 0
view get_verifier "{\"verifier\":\"$VERIFIER\"}"

say "2. acme hires nova (no history) for 0.5 NEAR + 0.025 NEAR verifier fee; the verifier accepts"
DEADLINE=$(( $(now_ns) + 20 * 60 * 1000000000 ))
call "$BUYER" post_job "{\"agent\":\"$NOVA\",\"verifier\":\"$VERIFIER\",\"spec\":\"sha256:slugify-24-cases\",\"payment\":\"$YOCTO_HALF\",\"deadline_ns\":$DEADLINE}" 0.525
call "$VERIFIER" accept_verification '{"job_id":0}' 0
view pass_price_bps '{"job_id":0}'

say "3. nova accepts with a 0.2 NEAR bond on PASS"
call "$NOVA" accept_job '{"job_id":0}' 0.2

say "4. acme buys cover (FAIL shares) for 0.1 NEAR"
call "$BUYER" buy '{"job_id":0,"outcome":"Fail"}' 0.1

say "5. Verifier attests two on-time heartbeats"
call "$VERIFIER" heartbeat '{"job_id":0,"on_time":true,"evidence":"sha256:commit-1"}' 0
call "$VERIFIER" heartbeat '{"job_id":0,"on_time":true,"evidence":"sha256:commit-2"}' 0
view pass_price_bps '{"job_id":0}'

say "6. Verifier runs the acceptance tests and resolves"
EVIDENCE=$(node "$ROOT/verifier/verify.js" "$ROOT/verifier/examples/slugify" "$ROOT/verifier/examples/slugify/good.js" | sed -n 's/^evidence: //p')
call "$VERIFIER" resolve "{\"job_id\":0,\"outcome\":\"Pass\",\"evidence\":\"$EVIDENCE\"}" 0
call "$NOVA" redeem '{"job_id":0}' 0
view get_agent "{\"agent\":\"$NOVA\"}"

say "7. globex hires sleeper"
DEADLINE=$(( $(now_ns) + 20 * 60 * 1000000000 ))
call "$GLOBEX" post_job "{\"agent\":\"$SLEEPER\",\"verifier\":\"$VERIFIER\",\"spec\":\"sha256:migration\",\"payment\":\"$YOCTO_HALF\",\"deadline_ns\":$DEADLINE}" 0.525
call "$VERIFIER" accept_verification '{"job_id":1}' 0
call "$SLEEPER" accept_job '{"job_id":1}' 0.05
call "$GLOBEX" buy '{"job_id":1,"outcome":"Fail"}' 0.15

say "8. sleeper tries to bet against itself (expect: Smart contract panicked: an agent cannot bet against its own work)"
call "$SLEEPER" buy '{"job_id":1,"outcome":"Fail"}' 0.05 || echo "refused, as designed"

say "9. Missed heartbeats"
call "$VERIFIER" heartbeat '{"job_id":1,"on_time":false,"evidence":"missed"}' 0
call "$VERIFIER" heartbeat '{"job_id":1,"on_time":false,"evidence":"missed"}' 0
view pass_price_bps '{"job_id":1}'

say "10. Nothing is delivered: the verifier rules FAIL (and is paid its fee either way)"
call "$VERIFIER" resolve '{"job_id":1,"outcome":"Fail","evidence":"no delivery"}' 0
call "$GLOBEX" redeem '{"job_id":1}' 0
view get_job '{"job_id":1}'
view get_vault '{}'
view get_verifier "{\"verifier\":\"$VERIFIER\"}"
view list_agent_listings '{}'

say "Done. Contract: $C"
[ "$NETWORK" = testnet ] && echo "Explorer: https://testnet.nearblocks.io/address/$C"
exit 0
