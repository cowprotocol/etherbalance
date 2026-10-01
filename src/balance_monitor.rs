use crate::config;
use anyhow::{anyhow, Context, Error, Result};
use ethcontract::dyns::DynTransport;
use std::{collections::HashMap, rc::Rc};
use url::Url;
use web3::{
    error::Error as Web3Error,
    types::{Address, U256},
    Transport,
};

#[derive(Clone, Debug)]
pub struct BalanceMonitor {
    networks: Vec<Network>,
}

#[derive(Debug)]
pub struct CallbackParameters<'a> {
    pub network_name: &'a str,
    pub address_name: &'a str,
    pub address: &'a AccountAddress,
    pub token_name: &'a str,
    pub balance: Result<U256>,
    pub tag: &'a str,
}

impl BalanceMonitor {
    pub fn new(config: config::Config) -> Result<Self> {
        if config
            .networks
            .iter()
            .flat_map(|network| network.tokens.iter())
            .any(|(name, _address)| name == "ether")
        {
            return Err(anyhow!(
                "token name ether is cannot be used for ERC20 tokens"
            ));
        }
        let networks = config
            .networks
            .into_iter()
            .map(create_network)
            .collect::<Result<_>>()?;
        Ok(Self { networks })
    }

    /// Retrieve all balances and call a function for each.
    pub async fn do_with_balances<T>(&self, callback: T)
    where
        T: Fn(CallbackParameters),
    {
        // TODO: batch requests
        for network in &self.networks {
            for address in &network.addresses {
                if address.monitor_ether {
                    let (token_name, balance) =
                        network.client.native_balance(&address.address).await;
                    callback(CallbackParameters {
                        network_name: &network.name,
                        address_name: &address.name,
                        address: &address.address,
                        token_name,
                        balance,
                        tag: &address.tag,
                    });
                }
                for token in &address.tokens {
                    let AccountAddress::Evm(account) = address.address else {
                        unreachable!("tokens are only configured for evm addresses")
                    };
                    let balance = erc20_balance(&token.contract, account).await;
                    callback(CallbackParameters {
                        network_name: &network.name,
                        address_name: &address.name,
                        address: &address.address,
                        token_name: &token.name,
                        balance: balance.map_err(Error::new),
                        tag: &address.tag,
                    });
                }
            }
        }
    }
}

fn create_transport(url: &Url) -> Result<DynTransport> {
    // TODO: transport with timeouts
    match url.scheme() {
        "http" | "https" => {
            let transport = web3::transports::Http::new(url.as_str())?;
            Ok(DynTransport::new(transport))
        }
        other => Err(anyhow!("unknown scheme: {}", other)),
    }
}

fn create_network(network: config::Network) -> Result<Network> {
    let url: Url = network.url.parse().context("invalid url")?;
    let transport = create_transport(&url).context("failed to create transport from node uri")?;
    let (client, addresses) = match network.kind {
        config::Kind::Evm => {
            let web3 = web3::Web3::new(transport);
            let tokens = create_tokens(network.tokens, &web3);
            let addresses = create_addresses_to_monitor(network.addresses, &tokens)?;
            (Client::Evm(web3), addresses)
        }
        config::Kind::Solana => {
            if !network.tokens.is_empty() {
                return Err(anyhow!(
                    "network {} is a solana network, tokens are not supported",
                    network.name
                ));
            }
            let addresses = create_solana_addresses_to_monitor(network.addresses)?;
            (Client::Solana(transport), addresses)
        }
    };
    Ok(Network {
        name: network.name,
        client,
        addresses,
    })
}

#[derive(Clone, Debug)]
struct Network {
    name: String,
    client: Client,
    addresses: Vec<AddressToMonitor>,
}

#[derive(Clone, Debug)]
enum Client {
    Evm(web3::Web3<DynTransport>),
    /// Solana nodes speak JSON-RPC too, so the web3 transport is reused to send
    /// raw Solana requests.
    Solana(DynTransport),
}

impl Client {
    /// The native balance of an address together with its token name.
    async fn native_balance(&self, address: &AccountAddress) -> (&'static str, Result<U256>) {
        match (self, address) {
            (Client::Evm(web3), AccountAddress::Evm(address)) => (
                "ether",
                ether_balance(*address, &web3.eth())
                    .await
                    .map_err(Error::new),
            ),
            (Client::Solana(transport), AccountAddress::Solana(address)) => {
                ("sol", solana_balance(transport, address).await)
            }
            // Addresses are created from their network's config in create_network.
            _ => unreachable!("address kind always matches its network kind"),
        }
    }
}

#[derive(Clone, Debug)]
pub enum AccountAddress {
    Evm(Address),
    Solana(String),
}

impl AccountAddress {
    /// The address as it appears in the prometheus "address" label.
    pub fn label(&self) -> String {
        match self {
            AccountAddress::Evm(address) => format!("{:#x}", address),
            AccountAddress::Solana(address) => address.clone(),
        }
    }
}

ethcontract::contract!("contracts/IERC20.json");

#[derive(Clone, Debug)]
struct Token {
    name: String,
    contract: IERC20,
}

#[derive(Clone, Debug)]
struct AddressToMonitor {
    name: String,
    address: AccountAddress,
    monitor_ether: bool,
    tokens: Vec<Rc<Token>>,
    tag: String,
}

fn create_tokens(
    tokens: HashMap<String, config::Address_>,
    web3: &web3::Web3<DynTransport>,
) -> HashMap<String, Rc<Token>> {
    tokens
        .into_iter()
        .map(|(name, address)| {
            (
                name.clone(),
                Rc::new(Token {
                    name,
                    contract: IERC20::at(web3, address.0),
                }),
            )
        })
        .collect()
}

fn create_addresses_to_monitor(
    addresses: HashMap<String, config::ConfigAddress>,
    tokens: &HashMap<String, Rc<Token>>,
) -> Result<Vec<AddressToMonitor>> {
    addresses
        .into_iter()
        .map(|(name, config_address)| {
            let tokens: Result<Vec<Rc<Token>>> = config_address
                .tokens
                .iter()
                .map(|name| {
                    tokens
                        .get(name)
                        .ok_or_else(|| anyhow!("token named {} not found", name))
                        .cloned()
                })
                .collect();
            let address = config::hex_string_to_address(&config_address.address)
                .with_context(|| format!("failed to parse address of {}", name))?;
            Ok(AddressToMonitor {
                name,
                address: AccountAddress::Evm(address),
                monitor_ether: config_address.ether,
                tokens: tokens?,
                tag: config_address.tag.unwrap_or_default(),
            })
        })
        .collect()
}

fn create_solana_addresses_to_monitor(
    addresses: HashMap<String, config::ConfigAddress>,
) -> Result<Vec<AddressToMonitor>> {
    addresses
        .into_iter()
        .map(|(name, config_address)| {
            if !config_address.tokens.is_empty() {
                return Err(anyhow!(
                    "address {} is on a solana network, tokens are not supported",
                    name
                ));
            }
            validate_solana_address(&config_address.address)
                .with_context(|| format!("failed to parse address of {}", name))?;
            Ok(AddressToMonitor {
                name,
                address: AccountAddress::Solana(config_address.address),
                monitor_ether: config_address.ether,
                tokens: Vec::new(),
                tag: config_address.tag.unwrap_or_default(),
            })
        })
        .collect()
}

fn validate_solana_address(address: &str) -> Result<()> {
    let bytes = bs58::decode(address)
        .into_vec()
        .with_context(|| format!("\"{}\" is not base58", address))?;
    if bytes.len() != 32 {
        return Err(anyhow!(
            "\"{}\" decodes to {} bytes instead of 32",
            address,
            bytes.len()
        ));
    }
    Ok(())
}

async fn ether_balance(
    address: Address,
    eth_api: &web3::api::Eth<impl Transport>,
) -> Result<U256, Web3Error> {
    eth_api.balance(address, None).await
}

async fn erc20_balance(
    contract: &IERC20,
    address: Address,
) -> Result<U256, ethcontract::errors::MethodError> {
    contract.balance_of(address).call().await
}

/// The SOL balance of an account in lamports.
async fn solana_balance(transport: &DynTransport, address: &str) -> Result<U256> {
    let response = transport
        .execute(
            "getBalance",
            vec![
                serde_json::json!(address),
                serde_json::json!({ "commitment": "confirmed" }),
            ],
        )
        .await?;
    let lamports = response["value"]
        .as_u64()
        .ok_or_else(|| anyhow!("unexpected getBalance response: {}", response))?;
    Ok(U256::from(lamports))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_solana_addresses() {
        assert!(validate_solana_address("Grr6SWYUFi1eCagwEifXVD83rUQ4W5rJWYq1Lj7cx1jS").is_ok());
        // Not base58 (contains 0).
        assert!(validate_solana_address("0rr6SWYUFi1eCagwEifXVD83rUQ4W5rJWYq1Lj7cx1jS").is_err());
        // Valid base58 but too short.
        assert!(validate_solana_address("Grr6SWYUFi1eCagw").is_err());
    }
}
