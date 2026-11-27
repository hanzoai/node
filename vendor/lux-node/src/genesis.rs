// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// See the file LICENSE for licensing terms.

//! Reading a genesis document.
//!
//! A genesis is data, and this is the one place it is turned into the value
//! `lux-evm` executes against. Keeping the parsing here rather than in the EVM
//! is the same separation Go has: the chain knows what an allocation MEANS, the
//! node knows what file it came out of.
//!
//! The document is the C-chain genesis a Go node is started with, and the two
//! shapes it comes in are both accepted, because they are both what is on disk:
//! the C-chain document itself, or a whole network genesis carrying it under
//! `cChainGenesis` — sometimes as an object, sometimes as an embedded JSON
//! string.
//!
//! WHAT IS REFUSED. A field that is present and unreadable is an error, never a
//! default. A genesis that silently fell back to zero for a balance it could
//! not parse would produce a state root that is wrong in a way nobody notices
//! until the chain forks.
//!
//! THE CONFIG IS READ AS `parseGenesis` READS IT (luxfi/evm plugin/evm/vm.go)
//! and `SetEthUpgrades` then aligns it (params/config_extra.go:96-150):
//! `feeConfig` or else `DefaultFeeConfig`, `allowFeeRecipients`,
//! `blobSchedule`, `skipPostMergeFields`; the Lux upgrades — `evmTimestamp`
//! and `durangoTimestamp` from genesis when absent, `feeSplitTimestamp`; the
//! Ethereum forks — Shanghai at Durango, Cancun at `quasarTimestamp` where
//! scheduled and else at `cancunTime`, both 0 when the document names neither;
//! and every stateful precompile a config key turns on, with the protocol's
//! system precompiles. Every block after the genesis is derived from these.
//!
//! A chain that turns on what this node does not run is refused here rather
//! than followed wrongly: a pre-EVM or pre-London genesis; Prague or later;
//! the fee manager, whose state replaces the fee schedule (GetFeeConfigAt);
//! the reward manager, whose state names the coinbase (GetCoinbaseAt); stake
//! weight, a second predicater; and every precompile whose `Configure` writes
//! state or can refuse (the allow lists, the native minter, the model
//! registry, the dead-address and FHE precompiles). Any other one is marked
//! when it turns on, as Go marks it, and a transaction that calls it is
//! refused (`lux_evm::precompile`).

use alloy_primitives::{Address, Bytes, B256, U256};
use lux_evm::fee;
use lux_evm::genesis::{Alloc, Genesis};
use lux_evm::precompile::{Activation, ALWAYS_ON};
use lux_evm::rules::Chain;
use serde_json::Value;

use crate::precompile::REGISTRY;

/// Precompile config keys whose `Configure` writes state or can refuse the
/// activation, and the predicaters: not run here.
const REFUSED: &[&str] = &[
    "feeManagerConfig",
    "rewardManagerConfig",
    "stakeWeightConfig",
    "txAllowListConfig",
    "contractDeployerAllowListConfig",
    "contractNativeMinterConfig",
    "modelRegistryConfig",
    "deadConfig",
    "deadFullConfig",
    "deadZeroConfig",
    "fheConfig",
    "bridgeRegistrarConfig",
];

/// Block-numbered forks: every one must be at 0, as every Lux chain has them.
const AT_ZERO: &[&str] = &[
    "homesteadBlock",
    "eip150Block",
    "eip155Block",
    "eip158Block",
    "byzantiumBlock",
    "constantinopleBlock",
    "petersburgBlock",
    "istanbulBlock",
    "muirGlacierBlock",
    "berlinBlock",
    "londonBlock",
];

/// Forks after Cancun: not run here.
const LATER: &[&str] = &["pragueTime", "osakaTime", "verkleTime"];

/// `UnscheduledActivationTime`: a Lux upgrade at or past it is not scheduled.
const UNSCHEDULED: u64 = i64::MAX as u64;

/// Apply a network's `upgrade.json` — luxd's `UpgradeConfig`, decoded with
/// unknown fields refused as luxd decodes it — to `genesis`.
///
/// It FAILS CLOSED: an entry this node does not run refuses the whole
/// document, so a node that cannot follow a chain says so when it starts
/// rather than diverging at the block the entry turns on. What it runs is a
/// precompile upgrade enabling one of the BLS12-381 modules lux-evm serves;
/// every other precompile, a disable, a state upgrade and a network-upgrade
/// override are refused.
pub fn upgrade(genesis: &mut Genesis, raw: &str) -> Result<(), String> {
    let doc: Value = serde_json::from_str(raw).map_err(|e| format!("not JSON: {e}"))?;
    let doc = doc.as_object().ok_or("an upgrade document is an object")?;
    for (field, value) in doc {
        match field.as_str() {
            "precompileUpgrades" => {}
            "stateUpgrades" if value.as_array().is_some_and(|a| a.is_empty()) => {}
            "stateUpgrades" => return Err("stateUpgrades are not run here".into()),
            "networkUpgradeOverrides" if value.is_null() => {}
            "networkUpgradeOverrides" => {
                return Err("networkUpgradeOverrides are not run here".into());
            }
            other => return Err(format!("{other} is not a field of an upgrade document")),
        }
    }
    let entries = match doc.get("precompileUpgrades") {
        None | Some(Value::Null) => return Ok(()),
        Some(Value::Array(a)) => a,
        Some(_) => return Err("precompileUpgrades is not a list".into()),
    };
    let mut previous: Option<u64> = None;
    for entry in entries {
        let (key, config) = match entry.as_object().map(|o| o.iter().collect::<Vec<_>>()) {
            Some(one) if one.len() == 1 => one[0],
            _ => return Err("a precompile upgrade names exactly one precompile".into()),
        };
        let address = crate::precompile::address_of(key)
            .ok_or_else(|| format!("{key} is not a precompile luxd registers"))?;
        let at = seconds(config, "blockTimestamp")?
            .ok_or_else(|| format!("{key} has no blockTimestamp"))?;
        if !enabled(&serde_json::json!({ key.as_str(): config }), key)? {
            return Err(format!("{key} at {at}: disabling a precompile is not run here"));
        }
        if !lux_evm::precompile::MODULES.iter().any(|m| m.address == address) {
            return Err(format!("{key} at {at}: a precompile this node does not run"));
        }
        // What luxd's `verifyPrecompileUpgrades` refuses of what is left
        // (params/extras/precompile_upgrade.go:77-150): an entry dated before
        // the one ahead of it, and turning on a precompile already on — with
        // no disable run here, any second enable of one.
        if let Some(before) = previous.filter(|&before| at < before) {
            return Err(format!("{key} at {at}: before the entry ahead of it, at {before}"));
        }
        if genesis.activations.iter().any(|a| a.address == address) {
            return Err(format!("{key} at {at}: already on"));
        }
        previous = Some(at);
        genesis.activations.push(Activation { address, at });
    }
    Ok(())
}

/// The precompiles `genesis` turns on, now or later, that lux-evm does not
/// run, each as its name, address and activation time. A transaction that
/// reaches one is refused here, in a block built or verified, and executed by
/// a Go validator — so a node must not share a validator set with Go
/// validators while one is active.
pub fn refused(genesis: &Genesis) -> Vec<String> {
    genesis
        .activations
        .iter()
        .filter(|a| !lux_evm::precompile::MODULES.iter().any(|m| m.address == a.address))
        .map(|a| format!("{} {} from {}", name_of(&a.address), a.address, a.at))
        .collect()
}

/// The name luxd registers the precompile at `address` under.
fn name_of(address: &Address) -> String {
    REGISTRY
        .iter()
        .find(|(_, a)| a.parse::<Address>().is_ok_and(|a| &a == address))
        .map(|(k, _)| (*k).to_string())
        .unwrap_or_else(|| format!("{address}"))
}

/// Read a C-chain genesis out of `raw`.
pub fn parse(raw: &str) -> Result<Genesis, String> {
    let doc: Value = serde_json::from_str(raw).map_err(|e| format!("not JSON: {e}"))?;
    parse_value(&doc)
}

fn parse_value(doc: &Value) -> Result<Genesis, String> {
    // A network genesis carries the C-chain's inside it, and Go writes that
    // field either as an object or as a string holding the document.
    if let Some(inner) = doc.get("cChainGenesis") {
        return match inner {
            Value::String(s) => parse(s),
            other => parse_value(other),
        };
    }

    let config = doc.get("config").ok_or("no config")?;
    let chain_id = config.get("chainId").and_then(|c| c.as_u64()).ok_or("no config.chainId")?;
    let timestamp = quantity(doc, "timestamp")?.unwrap_or(0);
    let chain = upgrades(config)?;
    if !chain.at(timestamp).evm {
        return Err("config.evmTimestamp is after the genesis; pre-EVM rules are not run".into());
    }
    let activations = activations(config)?;
    let blob = match config.get("blobSchedule").and_then(|b| b.get("cancun")) {
        None | Some(Value::Null) => fee::Blob::CANCUN,
        Some(v) => fee::Blob {
            target: seconds(v, "target")?.ok_or("config.blobSchedule.cancun has no target")?,
            max: seconds(v, "max")?.ok_or("config.blobSchedule.cancun has no max")?,
            fraction: seconds(v, "baseFeeUpdateFraction")?
                .ok_or("config.blobSchedule.cancun has no baseFeeUpdateFraction")?,
        },
    };
    let post_merge = match doc.get("skipPostMergeFields") {
        None | Some(Value::Null) => true,
        Some(Value::Bool(b)) => !b,
        Some(_) => return Err("skipPostMergeFields is not a boolean".into()),
    };
    for field in ["excessBlobGas", "blobGasUsed"] {
        if quantity(doc, field)?.is_some_and(|v| v != 0) {
            return Err(format!("{field} at the genesis is not run here"));
        }
    }
    let fee = match config.get("feeConfig") {
        None | Some(Value::Null) => fee::Config::DEFAULT,
        Some(v) => schedule(v).map_err(|e| format!("config.feeConfig: {e}"))?,
    };
    let allow_fee_recipients = match config.get("allowFeeRecipients") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err("config.allowFeeRecipients is not a boolean".into()),
    };

    let mut alloc = Vec::new();
    if let Some(entries) = doc.get("alloc").and_then(|a| a.as_object()) {
        for (who, entry) in entries {
            let address = address(who)?;
            alloc.push((address, one(entry).map_err(|e| format!("alloc {who}: {e}"))?));
        }
    }
    // The trie sorts on its own, but a stable order makes two dumps of one
    // genesis comparable.
    alloc.sort_by_key(|(a, _)| *a);

    Ok(Genesis {
        chain_id,
        chain,
        fee,
        blob,
        activations,
        allow_fee_recipients,
        post_merge,
        timestamp,
        gas_limit: quantity(doc, "gasLimit")?.ok_or("no gasLimit")?,
        difficulty: big(doc, "difficulty")?.unwrap_or(U256::ZERO),
        base_fee: quantity(doc, "baseFeePerGas")?.unwrap_or(25_000_000_000),
        coinbase: match doc.get("coinbase") {
            Some(v) => address(v.as_str().ok_or("coinbase is a string")?)?,
            None => Address::ZERO,
        },
        extra_data: match doc.get("extraData") {
            Some(v) => Bytes::from(data(v)?),
            None => Bytes::new(),
        },
        mix_hash: match doc.get("mixHash") {
            Some(v) => {
                let b = data(v)?;
                if b.len() != 32 {
                    return Err("mixHash is 32 bytes".into());
                }
                B256::from_slice(&b)
            }
            None => B256::ZERO,
        },
        nonce: quantity(doc, "nonce")?.unwrap_or(0),
        alloc,
    })
}

/// When each upgrade turns on. EVM and Durango are active from genesis unless
/// the document says otherwise; the fee split only when it names it. Shanghai
/// is Durango and Cancun is Quasar where those are scheduled
/// (`SetEthUpgrades`); otherwise the document's times stand, 0 where it names
/// none (`parseGenesis`).
fn upgrades(config: &Value) -> Result<Chain, String> {
    for key in AT_ZERO {
        if config.get(*key).is_some_and(|v| !v.is_null() && v.as_u64() != Some(0)) {
            return Err(format!(
                "config.{key} is not 0; only chains at London from genesis are run"
            ));
        }
    }
    for key in LATER {
        if config.get(*key).is_some_and(|v| !v.is_null()) {
            return Err(format!("config.{key}: forks after Cancun are not run here"));
        }
    }
    let durango = seconds(config, "durangoTimestamp")?.unwrap_or(0);
    let shanghai =
        if durango < UNSCHEDULED { durango } else { seconds(config, "shanghaiTime")?.unwrap_or(0) };
    let cancun = match seconds(config, "quasarTimestamp")? {
        Some(q) if q < UNSCHEDULED => q,
        _ => seconds(config, "cancunTime")?.unwrap_or(0),
    };
    if cancun < shanghai {
        return Err("Cancun before Shanghai".into());
    }
    Ok(Chain {
        evm: Some(seconds(config, "evmTimestamp")?.unwrap_or(0)),
        durango: Some(durango),
        split: seconds(config, "feeSplitTimestamp")?,
        shanghai: Some(shanghai),
        cancun: Some(cancun),
    })
}

/// Every stateful precompile the config turns on, at its `blockTimestamp`,
/// then the protocol's system precompiles. A key luxd does not register is
/// not a precompile config, and Go reads none from it either.
fn activations(config: &Value) -> Result<Vec<Activation>, String> {
    let mut out = Vec::new();
    for (key, address) in REGISTRY {
        if !enabled(config, key)? {
            continue;
        }
        if REFUSED.contains(key) {
            return Err(format!("config.{key} turns on a precompile this node does not run"));
        }
        let at = seconds(&config[*key], "blockTimestamp")?
            .ok_or_else(|| format!("config.{key} has no blockTimestamp"))?;
        out.push(Activation { address: address.parse().expect("a registry address literal"), at });
    }
    out.extend_from_slice(&ALWAYS_ON);
    Ok(out)
}

/// Whether the precompile config at `key` turns its precompile on: present,
/// and not `"disable": true`.
fn enabled(config: &Value, key: &str) -> Result<bool, String> {
    match config.get(key) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Object(o)) => match o.get("disable") {
            None | Some(Value::Null) | Some(Value::Bool(false)) => Ok(true),
            Some(Value::Bool(true)) => Ok(false),
            Some(_) => Err(format!("config.{key}.disable is not a boolean")),
        },
        Some(_) => Err(format!("config.{key} is not an object")),
    }
}

/// A timestamp in the config: a JSON number, as Go reads one.
fn seconds(at: &Value, field: &str) -> Result<Option<u64>, String> {
    match at.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => {
            n.as_u64().map(Some).ok_or_else(|| format!("{field} is not a whole number of seconds"))
        }
        Some(_) => Err(format!("{field} is not a number")),
    }
}

/// `feeConfig`: every field present, each a JSON number that fits the width Go
/// computes it at, and the whole passing `FeeConfig.Verify`.
fn schedule(v: &Value) -> Result<fee::Config, String> {
    let field = |name: &str| -> Result<u64, String> {
        match v.get(name) {
            Some(Value::Number(n)) => {
                n.as_u64().ok_or_else(|| format!("{name} is not a 64-bit whole number"))
            }
            Some(_) => Err(format!("{name} is not a number")),
            None => Err(format!("no {name}")),
        }
    };
    let config = fee::Config {
        gas_limit: field("gasLimit")?,
        target_block_rate: field("targetBlockRate")?,
        min_base_fee: field("minBaseFee")?,
        target_gas: field("targetGas")?,
        base_fee_change_denominator: field("baseFeeChangeDenominator")?,
        min_block_gas_cost: field("minBlockGasCost")?,
        max_block_gas_cost: field("maxBlockGasCost")?,
        block_gas_cost_step: field("blockGasCostStep")?,
    };
    config.verify()?;
    Ok(config)
}

/// One allocation entry.
fn one(entry: &Value) -> Result<Alloc, String> {
    let mut storage = Vec::new();
    if let Some(slots) = entry.get("storage").and_then(|s| s.as_object()) {
        for (slot, value) in slots {
            storage.push((word(slot)?, word(value.as_str().ok_or("a slot is a string")?)?));
        }
    }
    Ok(Alloc {
        balance: match entry.get("balance") {
            Some(v) => parse_u256(v.as_str().ok_or("balance is a string")?)?,
            None => U256::ZERO,
        },
        nonce: match entry.get("nonce") {
            Some(v) => parse_u64(v.as_str().ok_or("nonce is a string")?)?,
            None => 0,
        },
        code: match entry.get("code") {
            Some(v) => Bytes::from(data(v)?),
            None => Bytes::new(),
        },
        storage,
    })
}

fn quantity(doc: &Value, field: &str) -> Result<Option<u64>, String> {
    match doc.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => Ok(n.as_u64()),
        Some(Value::String(s)) => parse_u64(s).map(Some).map_err(|e| format!("{field}: {e}")),
        Some(_) => Err(format!("{field} is not a quantity")),
    }
}

fn big(doc: &Value, field: &str) -> Result<Option<U256>, String> {
    match doc.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => Ok(n.as_u64().map(U256::from)),
        Some(Value::String(s)) => parse_u256(s).map(Some).map_err(|e| format!("{field}: {e}")),
        Some(_) => Err(format!("{field} is not a quantity")),
    }
}

fn parse_u64(s: &str) -> Result<u64, String> {
    let raw = s.strip_prefix("0x").unwrap_or(s);
    u64::from_str_radix(raw, 16).map_err(|e| format!("{s}: {e}"))
}

fn parse_u256(s: &str) -> Result<U256, String> {
    let raw = s.strip_prefix("0x").unwrap_or(s);
    U256::from_str_radix(raw, 16).map_err(|e| format!("{s}: {e}"))
}

fn data(v: &Value) -> Result<Vec<u8>, String> {
    let s = v.as_str().ok_or("data is a string")?;
    alloy_primitives::hex::decode(s).map_err(|e| format!("{s}: {e}"))
}

/// An address, with or without the `0x` Go writes inconsistently.
fn address(s: &str) -> Result<Address, String> {
    let raw = s.strip_prefix("0x").unwrap_or(s);
    let bytes = alloy_primitives::hex::decode(raw).map_err(|e| format!("{s}: {e}"))?;
    if bytes.len() != 20 {
        return Err(format!("{s} is not 20 bytes"));
    }
    Ok(Address::from_slice(&bytes))
}

fn word(s: &str) -> Result<U256, String> {
    parse_u256(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::precompile;

    /// The C-chain genesis of the network a node on this machine runs, as that
    /// node was started with it (hanzoai/node genesis/mainnet.json's config).
    /// The whole point of this module is that a file like this produces the
    /// block that node reports.
    const LIVE: &str = r#"{
      "skipPostMergeFields": true,
      "config": {"chainId": 36963, "shanghaiTime": 0, "cancunTime": 0},
      "nonce": "0x0",
      "timestamp": "0x0",
      "extraData": "0x",
      "gasLimit": "0xb71b00",
      "difficulty": "0x0",
      "mixHash": "0x0000000000000000000000000000000000000000000000000000000000000000",
      "coinbase": "0x0000000000000000000000000000000000000000",
      "baseFeePerGas": "0x5d21dba00",
      "alloc": {
        "0200000000000000000000000000000000000005": {"balance": "0x0", "code": "0x01", "nonce": "0x1"},
        "0x703D4227d58d0b6A20BD721c940CED170470f634": {"balance": "0xd3c21bcecceda1000000"},
        "778237fd03e9b59f1803c87117a7b1cfbd7fc60e": {"balance": "0x3635c9adc5dea00000"},
        "0x9011E888251AB053B7bD1cdB598Db4f9DEd94714": {"balance": "0xde24aac7eb3e705b4e0000000"}
      }
    }"#;

    #[test]
    fn a_real_genesis_document_produces_the_block_the_node_reports() {
        let g = parse(LIVE).expect("parses");
        assert_eq!(g.chain_id, 36963);
        assert_eq!(g.alloc.len(), 4);
        assert_eq!(
            format!("{:#x}", g.world().root()),
            "0xe66e9ed33175518345f38838fdc70ba71cd4df8090eb2625ca298eb1b779449e",
            "the stateRoot a running node reports"
        );
        assert_eq!(
            format!("{:#x}", g.hash()),
            "0xb4a9545d78d14a5582f390db08112752637243b9e76d594dea51a34d8bc79dc1",
            "the block hash a running node reports"
        );
    }

    #[test]
    fn a_network_genesis_carrying_the_chain_as_a_string_is_read_through() {
        let wrapped = serde_json::json!({
            "networkID": 36963,
            "cChainGenesis": LIVE,
        });
        let g = parse(&wrapped.to_string()).expect("parses");
        assert_eq!(g.chain_id, 36963);
        assert_eq!(
            format!("{:#x}", g.hash()),
            "0xb4a9545d78d14a5582f390db08112752637243b9e76d594dea51a34d8bc79dc1"
        );
    }

    #[test]
    fn an_address_is_read_with_or_without_its_prefix() {
        assert_eq!(
            address("0x9011E888251AB053B7bD1cdB598Db4f9DEd94714").expect("prefixed"),
            address("9011E888251AB053B7bD1cdB598Db4f9DEd94714").expect("bare")
        );
    }

    #[test]
    fn a_field_that_cannot_be_read_is_an_error_and_not_a_default() {
        // The failure that would otherwise fork a chain quietly.
        let bad = LIVE.replace("\"0xd3c21bcecceda1000000\"", "\"0xnonsense\"");
        assert!(parse(&bad).is_err(), "a bad balance must not default to zero");

        let no_limit = LIVE.replace("\"gasLimit\": \"0xb71b00\",", "");
        assert!(parse(&no_limit).is_err(), "a missing gas limit is not zero");

        let no_id = LIVE.replace("\"chainId\": 36963,", "");
        assert!(parse(&no_id).is_err(), "a genesis with no chain id is not a genesis");
    }

    #[test]
    fn the_config_is_read_as_go_reads_it() {
        // luxd's localnet: its own schedule, warp from genesis.
        let luxd = parse(include_str!("../tests/vectors/luxd_localnet_cchain.json")).expect("luxd");
        assert_eq!(
            luxd.fee,
            fee::Config {
                gas_limit: 1_000_000_000,
                target_block_rate: 1,
                min_base_fee: 1_000_000_000,
                target_gas: 500_000_000,
                base_fee_change_denominator: 36,
                min_block_gas_cost: 0,
                max_block_gas_cost: 1_000_000,
                block_gas_cost_step: 200_000,
            }
        );
        assert_eq!(luxd.chain, Chain::lux());
        assert!(luxd.post_merge, "no skipPostMergeFields");
        let warp = Activation { address: lux_evm::rules::WARP, at: 0 };
        assert_eq!(luxd.activations, vec![warp, ALWAYS_ON[0]]);
        // Mainnet: the document this repo ships, and the values lux-evm carries.
        let main = parse(include_str!("../genesis/mainnet.json")).expect("mainnet");
        let built = lux_evm::genesis::mainnet();
        assert_eq!(
            (main.chain, main.fee, main.blob, &main.activations, main.post_merge),
            (built.chain, built.fee, built.blob, &built.activations, built.post_merge)
        );
        assert_eq!(main.chain.split, Some(1_789_430_400), "the fee split is genesis data");
        // It splits fees with no destination for the kept half, which luxd
        // v1.37.9 refuses to start on (fee_split.go:38-58); so does this node.
        let why = main.verify().expect_err("luxd refuses it");
        assert!(why.contains("feeSplitTimestamp"), "{why}");
        let names: Vec<String> = refused(&main)
            .iter()
            .map(|r| r.split(' ').next().unwrap_or_default().to_string())
            .collect();
        assert_eq!(names, ["warpConfig", "dexSettleConfig"], "what mainnet prints it refuses");
        // A config naming nothing runs Go's defaults: every fork from genesis,
        // Cancun included, and the system precompiles.
        let bare = parse(LIVE).expect("bare");
        assert_eq!(
            (bare.chain, bare.fee, bare.allow_fee_recipients, bare.activations),
            (Chain::lux(), fee::Config::DEFAULT, false, ALWAYS_ON.to_vec())
        );
        // Zoo's: Quasar scheduled in the year 9999 puts Cancun there, and
        // Durango at 0 puts Shanghai at genesis.
        let zoo = LIVE.replace(
            "\"chainId\": 36963, \"shanghaiTime\": 0, \"cancunTime\": 0",
            "\"chainId\": 36963, \"durangoTimestamp\": 0, \"quasarTimestamp\": 253399622400",
        );
        let zoo = parse(&zoo).expect("zoo");
        assert_eq!((zoo.chain.shanghai, zoo.chain.cancun), (Some(0), Some(253_399_622_400)));
    }

    #[test]
    fn a_config_this_node_does_not_run_is_refused() {
        let with = |key: &str, value: &str| {
            LIVE.replace("\"chainId\": 36963", &format!("\"chainId\": 36963, \"{key}\": {value}"))
        };
        for key in REFUSED {
            assert!(parse(&with(key, "{\"blockTimestamp\": 0}")).is_err(), "{key}");
            parse(&with(key, "{\"blockTimestamp\": 0, \"disable\": true}"))
                .expect("a disabled one is off");
        }
        assert!(parse(&with("evmTimestamp", "1")).is_err(), "pre-EVM rules are not run");
        assert!(parse(&with("allowFeeRecipients", "\"yes\"")).is_err());
        assert!(
            parse(&with("feeConfig", "{\"gasLimit\": 8000000}")).is_err(),
            "a partial schedule"
        );
        assert!(parse(&with("warpConfig", "{}")).is_err(), "warp with no activation");
        assert!(parse(&with("londonBlock", "5")).is_err(), "pre-London blocks are not run");
        assert!(parse(&with("pragueTime", "0")).is_err(), "Prague is not run");
        assert!(parse(&with("quasarTimestamp", "0")).is_ok(), "Quasar is Cancun");
        let ed = parse(&with("ed25519Config", "{\"blockTimestamp\": 7}")).expect("a marked one");
        let at = precompile::address_of("ed25519Config").expect("registered");
        assert!(ed.activations.contains(&Activation { address: at, at: 7 }));
        let recipients = parse(&with("allowFeeRecipients", "true")).expect("recipients");
        assert!(recipients.allow_fee_recipients);
    }

    #[test]
    fn an_upgrade_document_is_run_or_refused_whole() {
        let mut g = parse(LIVE).expect("genesis");
        let bls = r#"{"precompileUpgrades": [{"bls12381G1AddConfig": {"blockTimestamp": 5}}]}"#;
        upgrade(&mut g, bls).expect("a module this node serves");
        let at = precompile::address_of("bls12381G1AddConfig").expect("registered");
        assert!(g.activations.contains(&Activation { address: at, at: 5 }));
        for (doc, why) in [
            (r#"{"precompileUpgrades": [{"ed25519Config": {"blockTimestamp": 5}}]}"#, "not run"),
            (
                r#"{"precompileUpgrades": [{"bls12381G1AddConfig": {"blockTimestamp": 9, "disable": true}}]}"#,
                "disabling",
            ),
            (r#"{"stateUpgrades": [{"blockTimestamp": 5}]}"#, "stateUpgrades"),
            (r#"{"networkUpgradeOverrides": {"cancunTime": 5}}"#, "networkUpgradeOverrides"),
            (r#"{"_comment": "x"}"#, "not a field"),
            (
                r#"{"precompileUpgrades": [{"bls12381G1AddConfig": {"blockTimestamp": 9}},
                                           {"bls12381G1MulConfig": {"blockTimestamp": 5}}]}"#,
                "before the entry ahead",
            ),
            (
                r#"{"precompileUpgrades": [{"bls12381G1AddConfig": {"blockTimestamp": 5}},
                                           {"bls12381G1AddConfig": {"blockTimestamp": 9}}]}"#,
                "already on",
            ),
        ] {
            let refused = upgrade(&mut parse(LIVE).expect("genesis"), doc).expect_err(doc);
            assert!(refused.contains(why), "{doc}: {refused}");
        }
        // luxd's own localnet document turns on forty-four, most not run here.
        let luxd = include_str!("../tests/vectors/luxd_localnet_upgrade.json");
        assert!(upgrade(&mut parse(LIVE).expect("genesis"), luxd).is_err());
        assert_eq!(name_of(&lux_evm::rules::WARP), "warpConfig");
        // BLS is run here; the AlwaysOn DEX settlement precompile is not.
        let refused = refused(&g);
        assert_eq!(refused.len(), 1, "{refused:?}");
        assert!(refused[0].starts_with("dexSettleConfig"), "{refused:?}");
    }

    #[test]
    fn storage_in_an_allocation_reaches_the_state() {
        let doc = r#"{
          "config": {"chainId": 1},
          "gasLimit": "0x1",
          "alloc": {
            "0x0000000000000000000000000000000000000009": {
              "balance": "0x1", "nonce": "0x2", "code": "0xdead",
              "storage": {"0x01": "0x2a"}
            }
          }
        }"#;
        let g = parse(doc).expect("parses");
        let w = g.world();
        let who = address("0x0000000000000000000000000000000000000009").expect("addr");
        assert_eq!(w.nonce(&who), 2);
        assert_eq!(w.storage(&who, U256::from(1u64)), U256::from(42u64));
        assert_ne!(w.root(), lux_evm::root::EMPTY);
    }
}

#[cfg(test)]
mod luxd_conformance {
    use super::*;
    use crate::precompile;

    /// luxd's own localnet C-chain documents, copied verbatim from
    /// `luxfi/genesis@v1.16.19 configs/localnet/`.
    const LOCALNET: &str = include_str!("../tests/vectors/luxd_localnet_cchain.json");
    const UPGRADE: &str = include_str!("../tests/vectors/luxd_localnet_upgrade.json");

    /// What a live luxd 1.36.181 running those documents reports for block
    /// zero, captured over JSON-RPC.
    const LUXD_STATE_ROOT: &str =
        "0x497bcca8f4cee479610af8f1ddfa4e38036f5e9447366c2d0277ff70fcf3bdb9";
    const LUXD_GENESIS_HASH: &str =
        "0x6d6136e87748f9891d91c6f530a2ec09cdc40867089a012080ad5186df4704af";

    /// The precompile keys `upgrade.json` turns on, in document order.
    fn activated() -> Vec<String> {
        let doc: serde_json::Value = serde_json::from_str(UPGRADE).expect("upgrade.json parses");
        doc["precompileUpgrades"]
            .as_array()
            .expect("a list of upgrades")
            .iter()
            .filter_map(|e| e.as_object()?.keys().next().cloned())
            .collect()
    }

    #[test]
    fn luxds_genesis_document_is_read_the_way_luxd_reads_it() {
        let g = parse(LOCALNET).expect("luxd's genesis parses");
        assert_eq!(g.chain_id, 31337);
        assert_eq!(g.gas_limit, 0x3b9aca00);
        assert_eq!(g.base_fee, 0x3b9aca00);
        assert_eq!(g.timestamp, 0x6a065656);
        assert_eq!(g.alloc.len(), 2, "the document allocates two accounts");
        assert_eq!(activated().len(), 44, "and turns on forty-four precompiles");
    }

    #[test]
    fn the_genesis_state_is_the_allocation_and_the_system_precompiles() {
        // THE PREREQUISITE. Two nodes that disagree about block zero cannot
        // agree about block one, so this is the assertion co-certification
        // rests on: given luxd's own documents, this crate arrives at the state
        // root a live luxd reports.
        //
        // `upgrade.json`'s forty-four precompileUpgrades are NOT in it. They
        // are UPGRADES — each applies at the block transition crossing its
        // timestamp, and genesis has no transition to cross. Probing a live
        // node settles it: of the sixty registered modules, exactly two carry
        // code at block zero, and neither comes from that file. The warp
        // precompile is turned on by the config and is in the ALLOCATION,
        // spelled out by hand as `{balance 0, code 0x01, nonce 1}`; the DEX
        // settlement precompile is AlwaysOn, which the protocol turns on with
        // no config entry at all, and this genesis is after its time.
        let g = parse(LOCALNET).expect("parses");
        assert_eq!(format!("{:#x}", g.world().root()), LUXD_STATE_ROOT);
        // The allocation alone is a real number that a real node does NOT
        // report, and knowing that is what sent this looking for the rest.
        let bare = Genesis { activations: Vec::new(), ..g };
        assert_eq!(
            format!("{:#x}", bare.world().root()),
            "0x2d1cedac263020c5c56ef962f6abe0da1f5217bdc6468f8c9258a0ea23699e80",
            "the allocation's own root, which is what luxfi/genesis records for it"
        );
    }

    #[test]
    fn block_zero_is_named_what_luxd_names_it() {
        // `cchain.json` sets `cancunTime: 0` and no `skipPostMergeFields`, so
        // block zero carries the post-Shanghai tail: an empty withdrawals
        // root, zero blob gas, and a zero beacon root (core/genesis.go:468).
        let g = parse(LOCALNET).expect("parses");
        assert_eq!(g.sealed().fields(), 20, "core fifteen, base fee, and the Cancun tail");
        assert_eq!(format!("{:#x}", g.hash()), LUXD_GENESIS_HASH, "the hash a live luxd reports");
    }

    #[test]
    fn the_upgrades_document_is_not_genesis_state() {
        // Pinning the thing that was wrong, so it cannot come back: applying
        // upgrade.json's activations at genesis produces a root no Lux network
        // has ever had.
        let g = parse(LOCALNET).expect("parses");
        let mut wrong = g.world();
        // The document names modules by the keys luxd 1.36 registered; the
        // ones still registered are enough to move the root.
        for who in activated().iter().filter_map(|key| precompile::address_of(key)) {
            lux_evm::precompile::mark(&mut wrong, who);
        }
        assert_ne!(
            format!("{:#x}", wrong.root()),
            LUXD_STATE_ROOT,
            "upgrade.json at genesis is a chain nobody is running"
        );
    }
}

#[cfg(test)]
mod luxd_block_one {
    use alloy_consensus::Header;
    use alloy_primitives::{b256, Address, Bloom, Bytes, B256, B64, U256};
    use lux_evm::header::Lux;
    use lux_evm::root;

    /// Block one of the localnet C-chain a live luxd 1.36.181 produced, read
    /// back over JSON-RPC. Empty — no transactions — on top of the genesis
    /// this crate reproduces, so every field is either the parent's or a
    /// constant, and the hash is the whole claim.
    fn block_one() -> Lux {
        let header = Header {
            parent_hash: b256!("6d6136e87748f9891d91c6f530a2ec09cdc40867089a012080ad5186df4704af"),
            ommers_hash: b256!("1dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347"),
            beneficiary: "0x0100000000000000000000000000000000000000"
                .parse::<Address>()
                .expect("the coinbase luxd names"),
            state_root: b256!("497bcca8f4cee479610af8f1ddfa4e38036f5e9447366c2d0277ff70fcf3bdb9"),
            transactions_root: root::EMPTY,
            receipts_root: root::EMPTY,
            logs_bloom: Bloom::ZERO,
            // ONE, not zero. The genesis carries zero and the blocks after it
            // carry one; a node that assumed the genesis value names every
            // block wrongly.
            difficulty: U256::from(1u64),
            number: 1,
            gas_limit: 0x3b9aca00,
            gas_used: 0,
            timestamp: 0x6a98cfba,
            // Eighty-six zero bytes. Not empty, and not a round number —
            // exactly what the node wrote.
            extra_data: Bytes::from(vec![0u8; 86]),
            mix_hash: B256::ZERO,
            nonce: B64::ZERO,
            base_fee_per_gas: Some(0x3b9aca00),
            withdrawals_root: Some(root::EMPTY),
            blob_gas_used: Some(0),
            excess_blob_gas: Some(0),
            parent_beacon_block_root: Some(B256::ZERO),
            ..Default::default()
        };
        Lux::genesis(header)
    }

    #[test]
    fn this_node_names_luxds_block_one_the_way_luxd_names_it() {
        // Independent agreement on a block a live node PRODUCED, not just on
        // the genesis both were handed. The parent is the genesis this crate
        // computes from luxd's own documents, so the two chains agree at
        // height zero and at height one.
        assert_eq!(
            format!("{:#x}", block_one().seal()),
            "0x224357fadb81211f4bc11102799795c7ec6c3d1f5e645940ba3b5261858a43ab",
            "the hash a live luxd reports for block one"
        );
    }

    #[test]
    fn the_rpc_reports_a_field_the_hash_does_not_contain() {
        // `eth_getBlockByNumber` reports `blockGasCost: 0x0` for this block,
        // and the preimage does NOT carry it: the Cancun tail and the Lux
        // field are alternatives, not companions. The 20-field shape has no
        // position for one, so a header that believed the RPC cannot be named
        // in it at all.
        let one = block_one();
        assert_eq!((one.fields(), one.layout.name, one.block_gas_cost), (20, "20 eth", None));
        let mut with_cost = one;
        with_cost.block_gas_cost = Some(U256::ZERO);
        assert!(
            std::panic::catch_unwind(move || with_cost.seal()).is_err(),
            "believing the RPC here would name every block wrongly"
        );
    }

    #[test]
    fn luxds_block_one_is_the_one_the_rule_derives() {
        // A block a live Go node BUILT, held to this node's header rule on top
        // of the genesis both start from: the coinbase, difficulty, gas limit,
        // 86 bytes of extra, base fee and block gas cost are all what Go
        // derived, and this crate derives the same.
        let g = super::parse(include_str!("../tests/vectors/luxd_localnet_cchain.json"))
            .expect("luxd's genesis");
        let env = lux_evm::block::rule(&g.env(), &g.sealed(), None, &block_one(), None)
            .expect("luxd's block one passes");
        assert_eq!(env.base_fee, 0x3b9aca00);
        assert_eq!(env.block_gas_cost, 0);
    }

    #[test]
    fn an_empty_block_keeps_its_parents_state_root() {
        // Why this block is the right one to check first: nothing ran, so the
        // state root is the genesis root, and any disagreement is purely about
        // how a header is named rather than about execution.
        assert_eq!(
            block_one().header.state_root,
            b256!("497bcca8f4cee479610af8f1ddfa4e38036f5e9447366c2d0277ff70fcf3bdb9")
        );
    }
}

#[cfg(test)]
mod shared_network {
    use super::*;

    /// The C-chain genesis of the two-validator network luxd and this node
    /// were both started on: luxd's own localnet document, plus one account
    /// whose key this side holds so a transaction can actually be signed.
    const SHARED: &str = include_str!("../tests/vectors/shared_cchain.json");

    /// What the live luxd running it reports for block zero.
    const LUXD_STATE_ROOT: &str =
        "0x0a507b6bf97f05abf6fd5d98e3cfc3b20895d4543f6ab7c7b6cf82f294b949cf";
    const LUXD_GENESIS_HASH: &str =
        "0xb1a2d458a0d55bd47731e95e0626a9808791fa61326c54cbfb3699a600ea1aed";

    #[test]
    fn both_nodes_start_from_the_same_state() {
        // The shared genesis, computed here and reported there. Same three
        // accounts, same system precompile, same root — which is what makes a
        // vote about a block on top of it mean the same thing on both sides.
        let g = parse(SHARED).expect("the shared genesis parses");
        assert_eq!(g.chain_id, 31337);
        assert_eq!(g.alloc.len(), 3, "two from luxd's localnet, one this side can sign for");
        assert_eq!(format!("{:#x}", g.world().root()), LUXD_STATE_ROOT);
    }

    #[test]
    fn both_nodes_name_block_zero_the_same() {
        let g = parse(SHARED).expect("parses");
        assert_eq!(format!("{:#x}", g.hash()), LUXD_GENESIS_HASH);
    }
}
