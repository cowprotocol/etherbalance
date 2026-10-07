use std::{collections::HashMap, time::Duration};

use anyhow::{Context as _, Result};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use cow_settlement_interface::{
    data::{
        intent::{OrderIntent, OrderKind},
        order::{FillAmounts, OrderAccount, DISCRIMINATOR},
    },
    Pubkey,
};
use serde::Deserialize;
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};
use web3::Transport;

/// Default timeout for the expensive `getProgramAccounts` scan.
pub const SCAN_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Status {
    Open,
    Expired,
    Cancelled,
    Filled,
    Malformed,
}

impl Status {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Expired => "expired",
            Self::Cancelled => "cancelled",
            Self::Filled => "filled",
            Self::Malformed => "malformed",
        }
    }

    pub const ALL: [Self; 5] = [
        Self::Open,
        Self::Expired,
        Self::Cancelled,
        Self::Filled,
        Self::Malformed,
    ];
}

#[derive(Debug, Default)]
pub struct Stats {
    /// Number of order PDAs classified into each status.
    pub counts: HashMap<Status, u64>,
    /// Sum of rent lamports held by order PDAs of each status.
    pub rent: HashMap<Status, u64>,
    /// Sum of lamports that the `ReclaimOrder` instruction could recover now.
    pub reclaimable: u64,
}

/// One entry of a `getProgramAccounts` response.
#[derive(Debug, Deserialize)]
struct PubkeyAccount {
    #[allow(dead_code)]
    pubkey: String,
    account: RpcAccount,
}

#[derive(Debug, Deserialize)]
struct RpcAccount {
    data: (String, String),
    #[allow(dead_code)]
    owner: String,
    lamports: u64,
}

/// Scan all settlement order PDAs and aggregate their rent by status.
///
/// The scan is wrapped in a tokio timeout because `getProgramAccounts` can hang
/// on nodes that do not index it efficiently.
pub async fn scan<T>(transport: &T, program_id: &Pubkey, timeout: Duration) -> Result<Stats>
where
    T: Transport,
{
    tokio::time::timeout(timeout, scan_inner(transport, program_id))
        .await
        .map_err(|_| anyhow::anyhow!("order scan timed out after {timeout:?}"))?
}

async fn scan_inner<T>(transport: &T, program_id: &Pubkey) -> Result<Stats>
where
    T: Transport,
{
    let now = chain_now(transport).await?;

    let program_id_param = json!(program_id.to_string());
    let config_param = json!({
        "encoding": "base64",
        "filters": [{
            "memcmp": {
                "offset": 0,
                "bytes": BASE64.encode([DISCRIMINATOR]),
                "encoding": "base64",
            }
        }],
    });

    let response = transport
        .execute("getProgramAccounts", vec![program_id_param, config_param])
        .await
        .context("getProgramAccounts RPC call failed")?;
    let entries: Vec<PubkeyAccount> =
        serde_json::from_value(response).context("unexpected getProgramAccounts response shape")?;

    let mut stats = Stats::default();
    for entry in entries {
        let data = BASE64
            .decode(&entry.account.data.0)
            .context("failed to base64-decode account data")?;
        let (order_status, reclaimable) = classify(&data, now);
        *stats.counts.entry(order_status).or_default() += 1;
        *stats.rent.entry(order_status).or_default() += entry.account.lamports;
        if reclaimable {
            stats.reclaimable += entry.account.lamports;
        }
    }
    Ok(stats)
}

/// Cluster time, used to decide whether an order has expired.
///
/// Tries `getSlot` + `getBlockTime` first, falling back to wall time if the
/// cluster returns no block time for the latest slot.
async fn chain_now<T>(transport: &T) -> Result<i64>
where
    T: Transport,
{
    let slot: u64 = serde_json::from_value(
        transport
            .execute("getSlot", vec![json!({"commitment": "confirmed"})])
            .await
            .context("getSlot RPC call failed")?,
    )
    .context("unexpected getSlot response")?;

    let block_time: Option<i64> = serde_json::from_value(
        transport
            .execute("getBlockTime", vec![json!(slot)])
            .await
            .context("getBlockTime RPC call failed")?,
    )
    .context("unexpected getBlockTime response")?;

    Ok(block_time.unwrap_or_else(|| {
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before epoch")
            .as_secs();
        i64::try_from(seconds).unwrap_or(i64::MAX)
    }))
}

/// Classify an order body into a single exclusive status and whether it is
/// reclaimable right now according to the on-chain `ReclaimOrder` rule.
fn classify(data: &[u8], now: i64) -> (Status, bool) {
    let Ok(order) = OrderAccount::attach(data) else {
        return (Status::Malformed, false);
    };

    let Ok(cancelled) = order.cancelled() else {
        return (Status::Malformed, false);
    };

    let Ok(intent) = OrderIntent::try_from(order.intent_bytes()) else {
        return (Status::Malformed, false);
    };

    let fill = order.filled_amounts();
    let expired = now > i64::from(intent.valid_to);
    let filled = is_filled(&intent, &fill);
    let on_chain = intent.flags.created_on_chain;

    if expired {
        return (Status::Expired, true);
    }
    if cancelled {
        return (Status::Cancelled, on_chain);
    }
    if filled {
        return (Status::Filled, on_chain);
    }
    (Status::Open, false)
}

fn is_filled(intent: &OrderIntent, fill: &FillAmounts) -> bool {
    match intent.flags.kind {
        OrderKind::Buy => fill.received >= intent.buy_amount,
        OrderKind::Sell => fill.withdrawn >= intent.sell_amount,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cow_settlement_interface::data::{
        intent::{fixtures::sample_intent, Flags, OrderKind},
        order::fixtures::sample_order_bytes,
    };

    fn order_bytes(
        valid_to: u32,
        flags: Flags,
        cancelled: bool,
        withdrawn: u64,
        received: u64,
    ) -> Vec<u8> {
        let intent = OrderIntent {
            valid_to,
            flags,
            ..sample_intent(flags)
        };
        let mut bytes = [0u8; 264];
        OrderAccount::initialize(
            &mut bytes[..],
            0,
            cancelled,
            withdrawn,
            received,
            &Pubkey::new_from_array([0x42; 32]),
            &cow_settlement_interface::data::intent::EncodedOrderIntent::from(&intent),
        )
        .expect("initialize should succeed");
        bytes.to_vec()
    }

    fn sell_intent() -> OrderIntent {
        sample_intent(Flags {
            created_on_chain: false,
            kind: OrderKind::Sell,
            partially_fillable: true,
        })
    }

    fn sell_flags() -> Flags {
        Flags {
            created_on_chain: false,
            kind: OrderKind::Sell,
            partially_fillable: true,
        }
    }

    fn on_chain_sell_flags() -> Flags {
        Flags {
            created_on_chain: true,
            kind: OrderKind::Sell,
            partially_fillable: true,
        }
    }

    #[test]
    fn expired_wins_over_cancelled_and_filled() {
        let sell = sell_intent();
        let data = order_bytes(50, sell_flags(), true, sell.sell_amount, 0);
        let (status, reclaimable) = classify(&data, 51);
        assert_eq!(status, Status::Expired);
        assert!(reclaimable);
    }

    #[test]
    fn on_chain_cancelled_is_reclaimable_before_expiry() {
        let data = order_bytes(100, on_chain_sell_flags(), true, 0, 0);
        let (status, reclaimable) = classify(&data, 50);
        assert_eq!(status, Status::Cancelled);
        assert!(reclaimable);
    }

    #[test]
    fn off_chain_cancelled_is_not_reclaimable_before_expiry() {
        let data = order_bytes(100, sell_flags(), true, 0, 0);
        let (status, reclaimable) = classify(&data, 50);
        assert_eq!(status, Status::Cancelled);
        assert!(!reclaimable);
    }

    #[test]
    fn on_chain_filled_is_reclaimable_before_expiry() {
        let sell = sell_intent();
        let data = order_bytes(100, on_chain_sell_flags(), false, sell.sell_amount, 0);
        let (status, reclaimable) = classify(&data, 50);
        assert_eq!(status, Status::Filled);
        assert!(reclaimable);
    }

    #[test]
    fn partially_filled_stays_open() {
        let sell = sell_intent();
        let data = order_bytes(100, sell_flags(), false, sell.sell_amount - 1, 0);
        let (status, reclaimable) = classify(&data, 50);
        assert_eq!(status, Status::Open);
        assert!(!reclaimable);
    }

    #[test]
    fn buy_kind_fill_uses_received_amount() {
        let intent = sample_intent(Flags {
            created_on_chain: false,
            kind: OrderKind::Buy,
            partially_fillable: true,
        });
        let flags = Flags {
            created_on_chain: false,
            kind: OrderKind::Buy,
            partially_fillable: true,
        };
        let data = order_bytes(100, flags, false, 0, intent.buy_amount);
        let (status, reclaimable) = classify(&data, 50);
        assert_eq!(status, Status::Filled);
        assert!(!reclaimable);
    }

    #[test]
    fn malformed_short_body() {
        let (status, reclaimable) = classify(&[0u8; 10], 50);
        assert_eq!(status, Status::Malformed);
        assert!(!reclaimable);
    }

    #[test]
    fn malformed_bad_cancelled_byte() {
        let mut bytes = sample_order_bytes(false).to_vec();
        bytes[2] = 2;
        let (status, reclaimable) = classify(&bytes, 50);
        assert_eq!(status, Status::Malformed);
        assert!(!reclaimable);
    }

    #[test]
    fn malformed_reserved_flags_byte() {
        let mut bytes = sample_order_bytes(false).to_vec();
        bytes[51 + cow_settlement_interface::data::intent::fixtures::FLAGS_OFFSET] = 0xff;
        let (status, reclaimable) = classify(&bytes, 50);
        assert_eq!(status, Status::Malformed);
        assert!(!reclaimable);
    }

    #[test]
    fn valid_to_not_yet_expired() {
        let data = order_bytes(50, sell_flags(), false, 0, 0);
        let (status, reclaimable) = classify(&data, 50);
        assert_eq!(status, Status::Open);
        assert!(!reclaimable);
    }
}
