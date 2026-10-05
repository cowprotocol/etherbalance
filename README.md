# etherbalance

An ethereum ether and [ERC20](https://eips.ethereum.org/EIPS/eip-20) token balance monitoring application.
It can also monitor native SOL balances on Solana networks (`kind = 'solana'`), reported in lamports with `token_name="sol"`.

See the [example config file](example_config.toml), and command line options (`cargo run -- --help`):

```
FLAGS:
    -h, --help              Prints help information
        --print-balances    Print balances to stdout on update
    -V, --version           Prints version information

OPTIONS:
        --bind <bind>                          Serve the prometheus metrics at this address [default: 0.0.0.0:8080]
        --config <config>                      Path to the config file
        --node <node>                          Url of the ethereum node to communicate with
        --update-interval <update-interval>    Update the balances in this interval in seconds [default: 100]
```

The balance information is exposed as a prometheus metric at `/metrics`. With the example config file:

```
# HELP etherbalance_balance The ether or IERC20 balance of an ethereum address.
# TYPE etherbalance_balance gauge
etherbalance_balance{address="0x3f5ce5fbfe3e9af3971dd833d26ba9b5c936f0be",address_name="company-wallet",token_name="ether",tag=""} 208965276689158900000000
etherbalance_balance{address="0x3f5ce5fbfe3e9af3971dd833d26ba9b5c936f0be",address_name="company-wallet",token_name="usdc",tag=""} 16234719511522
etherbalance_balance{address="0x3f5ce5fbfe3e9af3971dd833d26ba9b5c936f0be",address_name="company-wallet",token_name="usdt",tag=""} 110017919015055
etherbalance_balance{address="0xbe0eb53f46cd790cd13851d5eff43d12404d33e8",address_name="personal-wallet",token_name="ether",tag="tag"} 2318528098086858200000000
etherbalance_balance{address="0xbe0eb53f46cd790cd13851d5eff43d12404d33e8",address_name="personal-wallet",token_name="usdc",tag="tag"} 49434572690562
etherbalance_balance{address="0xbe0eb53f46cd790cd13851d5eff43d12404d33e8",address_name="personal-wallet",token_name="usdt",tag="tag"} 717944090350919
# HELP etherbalance_last_update Unix time of last update of balances.
# TYPE etherbalance_last_update gauge
etherbalance_last_update 1586257843.7633243
```

And additionally on stdout with `--print-balances`:

```
address personal-wallet ether balance is 2318528098086858312934419
address personal-wallet usdt balance is 717944090350919
address personal-wallet usdc balance is 49434572690562
address company-wallet ether balance is 208965276689158926934455
address company-wallet usdt balance is 110017919015055
address company-wallet usdc balance is 16234719511522
```

This information is updated in the background with the specified
`--update-interval`. It is not updated on metric request as is custom for
Prometheus metrics because we want to avoid overloading the ethereum node.

# Development

[contracts/IERC20.sol](contracts/IERC20.sol) is [OpenZeppelin's version of IERC20](https://github.com/OpenZeppelin/openzeppelin-contracts/tree/master/contracts/token/ERC20).
It is used to generate [contracts/IERC20.json](contracts/IERC20.json) which in
turn is used by ethcontract-rs to generate the rust smart contract bindings.
This file is included in the repository instead of being generated when building
to make building as easy as possible. The smart contract is not expected to
change so the file will not need updating.