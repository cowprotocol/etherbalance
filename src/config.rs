use anyhow::{anyhow, Context as _, Result};
use serde::{de::Error as _, Deserialize, Deserializer};
use std::collections::HashMap;
use web3::types::Address;

/// Wrapper type of Address that implements deserialize from hex string.
#[derive(Debug)]
pub struct Address_(pub Address);

// Copied from ethcontract-rs.
pub fn hex_string_to_address(string: &str) -> Result<Address> {
    let prefix = "0x";
    if !string.starts_with(prefix) {
        return Err(anyhow!("does not start with {}", prefix));
    }
    Ok(string[2..].parse()?)
}

impl<'de> Deserialize<'de> for Address_ {
    fn deserialize<D>(deserializer: D) -> Result<Address_, D::Error>
    where
        D: Deserializer<'de>,
    {
        let string = String::deserialize(deserializer)?;
        let address = hex_string_to_address(&string)
            .with_context(|| format!("failed to parse address \"{}\"", string))
            .map_err(D::Error::custom)?;
        Ok(Address_(address))
    }
}

#[derive(Debug, Deserialize)]
pub struct ConfigAddress {
    /// Hex string for EVM networks, base58 string for Solana networks.
    pub address: String,
    /// Whether the native balance (ether, SOL, ...) should be monitored.
    pub ether: bool,
    pub tokens: Vec<String>,
    pub tag: Option<String>,
}

/// The kind of chain a network is, which determines how balances are fetched.
#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Evm,
    Solana,
}

#[derive(Debug, Deserialize)]
pub struct SettlementConfig {
    /// Solana program ID to scan for settlement order accounts.
    /// Defaults to the deployed `CoW` Protocol settlement program.
    pub program_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Network {
    #[serde(default)]
    pub kind: Kind,
    pub name: String,
    pub url: String,
    pub tokens: HashMap<String, Address_>,
    pub addresses: HashMap<String, ConfigAddress>,
    pub settlement: Option<SettlementConfig>,
}

/// The user facing config file.
#[derive(Debug, Deserialize)]
pub struct Config {
    pub networks: Vec<Network>,
}
