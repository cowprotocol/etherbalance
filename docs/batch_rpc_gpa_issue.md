# Batch RPC `getProgramAccounts` issue

## TL;DR

The Solana JSON-RPC node `https://ovh-solana-01.nodes.batch.exchange/rpc` ("Batch node") answers cheap calls such as `getSlot` and `getAccountInfo` quickly, but `getProgramAccounts` hangs and eventually times out for **any** program ID — including the CoW Protocol settlement program and the tiny `Config1111111111111111111111111111111111111` program. The same calls against the public `https://api.mainnet-beta.solana.com` return instantly.

This prevents the new etherbalance order-rent monitor from working with that node, because the monitor discovers order PDAs via `getProgramAccounts`.

## Nodes tested

| node | URL |
|------|-----|
| Batch | `https://ovh-solana-01.nodes.batch.exchange/rpc` |
| Public mainnet | `https://api.mainnet-beta.solana.com` |

## Quick comparison

| method | Batch | Public |
|--------|-------|--------|
| `getSlot` | ~0.7s ✅ | ~0.5s ✅ |
| `getAccountInfo(CoW program)` | ~0.7s ✅ | ~0.5s ✅ |
| `getProgramAccounts(CoW program)` | 35s timeout ❌ | ~0.7s ✅ |
| `getProgramAccounts(Config111)` | 35s timeout ❌ | ~0.5s ✅ |

## Minimal reproduction

All commands use plain `curl` and a 35-second timeout so the issue is easy to reproduce without running the full service.

### 1. Cheap calls work on both nodes

```bash
BATCH="https://ovh-solana-01.nodes.batch.exchange/rpc"
PUB="https://api.mainnet-beta.solana.com"

# getSlot
curl -s --max-time 10 -X POST "$BATCH" \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"getSlot"}'

curl -s --max-time 10 -X POST "$PUB" \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"getSlot"}'
```

Actual output:

```json
{"jsonrpc":"2.0","result":454263020,"id":1}   # Batch, ~0.72s
{"jsonrpc":"2.0","result":454263023,"id":1}   # Public, ~0.48s
```

```bash
# getAccountInfo for the CoW settlement program
PROG="C7PXyLpLQBh3Ce7e9DNj3rDVUvwqa5orDwQG5hs1rfNi"
curl -s --max-time 10 -X POST "$BATCH" \
  -H 'Content-Type: application/json' \
  -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getAccountInfo\",\"params\":[\"$PROG\",{\"encoding\":\"base64\"}]}"

curl -s --max-time 10 -X POST "$PUB" \
  -H 'Content-Type: application/json' \
  -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getAccountInfo\",\"params\":[\"$PROG\",{\"encoding\":\"base64\"}]}"
```

Actual output:

```json
{"jsonrpc":"2.0","result":{"context":{"apiVersion":"4.2.2","slot":454263025},"value":{"data":["AgAAAG1KHFNuFRuVQ0Jze3SMVGRm2yQ3i6QyYw7vTDsDIbM6","base64"],"executable":true,"lamports":833120,"owner":"BPFLoaderUpgradeab1e11111111111111111111111","rentEpoch":18446744073709551615,"space":36}},"id":1}
```

Both nodes return the same account data in under a second.

### 2. `getProgramAccounts` for the settlement program

```bash
BATCH="https://ovh-solana-01.nodes.batch.exchange/rpc"
PUB="https://api.mainnet-beta.solana.com"
PROG="C7PXyLpLQBh3Ce7e9DNj3rDVUvwqa5orDwQG5hs1rfNi"

# Batch node
curl -s --max-time 35 -X POST "$BATCH" \
  -H 'Content-Type: application/json' \
  -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getProgramAccounts\",\"params\":[\"$PROG\",{\"encoding\":\"base64\",\"filters\":[{\"memcmp\":{\"offset\":0,\"bytes\":\"gA==\",\"encoding\":\"base64\"}}]}]}"

# Public node
curl -s --max-time 35 -X POST "$PUB" \
  -H 'Content-Type: application/json' \
  -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getProgramAccounts\",\"params\":[\"$PROG\",{\"encoding\":\"base64\",\"filters\":[{\"memcmp\":{\"offset\":0,\"bytes\":\"gA==\",\"encoding\":\"base64\"}}]}]}" | head -c 500
```

Actual output:

```text
# Batch: no body, curl exits 28 (timeout) after 35.04s

# Public: returns immediately (~0.69s)
{"jsonrpc":"2.0","result":[{"pubkey":"12kcT9vNUynk4v7MMmG2SbKudt5S7myqCjepecJ86yyu","account":{"lamports":1991360,"data":["gPsAAAAAAAAAAAAAAAAAAAAAAC8q9A1dPXJFyiaKIIsTytVyAlonOEz5x0bzd+CwPPpk...","base64"],"owner":"C7PXyLpLQBh3Ce7e9DNj3rDVUvwqa5orDwQG5hs1rfNi"...
```

### 3. Control: a tiny program

To check whether the issue is specific to the settlement program or a general `getProgramAccounts` problem, call it for `Config1111111111111111111111111111111111111`, which has very few (or zero) accounts:

```bash
BATCH="https://ovh-solana-01.nodes.batch.exchange/rpc"
PUB="https://api.mainnet-beta.solana.com"
TINY="Config1111111111111111111111111111111111111"

# Batch node
curl -s --max-time 35 -X POST "$BATCH" \
  -H 'Content-Type: application/json' \
  -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getProgramAccounts\",\"params\":[\"$TINY\",{\"encoding\":\"base64\",\"filters\":[{\"dataSize\":76}]}]}"

# Public node
curl -s --max-time 35 -X POST "$PUB" \
  -H 'Content-Type: application/json' \
  -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getProgramAccounts\",\"params\":[\"$TINY\",{\"encoding\":\"base64\",\"filters\":[{\"dataSize\":76}]}]}"
```

Actual output:

```text
# Batch: no body, curl exits 28 (timeout) after 35.04s

# Public: returns immediately (~0.48s)
{"jsonrpc":"2.0","result":[],"id":1}
```

## Analysis

The Batch node fails `getProgramAccounts` even for the tiny `Config111...` program, so the issue is **method-level**, not settlement-program-specific. The most likely causes are:

1. **No program-ID account index.** Self-hosted RPCs need to explicitly enable indexing of program IDs (`--account-index-include-program-id` in agave/validator config) for `getProgramAccounts` to be fast. Without it, the node scans the entire accounts database, which can take minutes or longer.
2. **`getProgramAccounts` disabled or heavily throttled** at the proxy/node level for resource protection.
3. The node is primarily a **validator** with JSON-RPC exposed but without RPC indexing enabled.

## Impact on etherbalance

The new order-rent monitor calls `getProgramAccounts` once per update cycle to enumerate all order PDAs. When pointed at the Batch node, the call never completes within the 120-second tokio timeout, so:

- `etherbalance_order_rent_lamports` and `etherbalance_order_reclaimable_lamports` are not populated.
- `success_counter{result="failure", address="orders:<program_id>"}` increments.
- The monitor logs: `failed to scan order rent ...: order scan timed out after 120s`.

## Recommendation

Point the Solana network URL in `config.local.toml` at an RPC that indexes program accounts. The public `https://api.mainnet-beta.solana.com` works for testing, but for production use a dedicated paid RPC endpoint with `getProgramAccounts` support.
