// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// See the file LICENSE for licensing terms.

//! Stateful precompiles, and the state their activation writes.
//!
//! A Lux chain's genesis state is NOT only its allocation. `upgrade.json` turns
//! on stateful precompiles, and turning one on writes to state — so a node that
//! reads the allocation and stops computes a genesis root no Lux network has.
//! That is not a subtlety; it is the difference between agreeing with a live
//! node about block zero and disagreeing with it about everything.
//!
//! What activation writes, from `luxfi/evm@v1.104.50
//! core/state_processor_ext.go ApplyPrecompileActivations`:
//!
//! ```text
//! statedb.SetNonce(module.Address, 1)
//! statedb.SetCode(module.Address, []byte{0x1})
//! module.Configure(...)
//! ```
//!
//! The nonce and the one code byte are the marker: they make the account
//! non-empty so EIP-161 does not sweep it away, and they make `EXTCODESIZE`
//! non-zero so Solidity's contract-existence guard lets a call through. The
//! allocation of every Lux network already carries `0x02…05` in exactly that
//! shape — `{balance 0, code 0x01, nonce 1}` — because the warp precompile is
//! the one the document spells out by hand.
//!
//! WHEN IT HAPPENS AT GENESIS. The same file's comment is explicit: with no
//! parent timestamp, a module is crossed iff `ActivationTime <= blockTimestamp`,
//! "so the marker lands in the committed genesis state root ONLY for a chain
//! genesised at/after ActivationTime". A chain older than a system precompile
//! keeps its original genesis hash.
//!
//! THE TABLE IS LUXD'S OWN. Every pair below was read out of a running
//! `modules.RegisteredModules()` rather than transcribed from a document, so a
//! precompile that moves address moves here by re-reading the registry. The
//! genesis reader turns the config keys into `lux_evm::precompile::Activation`s,
//! and `lux_evm` writes the marker when each is crossed — at the genesis, or
//! at the block whose transition crosses it.

use alloy_primitives::Address;

/// Every module luxd registers, by the key a config names it with.
///
/// Read from `modules.RegisteredModules()` in luxfi/evm v1.104.56 — the
/// version luxd v1.37.9 links — with `precompile/registry` imported, as its VM
/// imports it.
pub const REGISTRY: &[(&str, &str)] = &[
    ("aiInferenceConfig", "0x0300000000000000000000000000000000000003"),
    ("aiMiningConfig", "0x0300000000000000000000000000000000000000"),
    ("aivmBridgeConfig", "0x0300000000000000000000000000000000000004"),
    ("anchorConfig", "0x0700000000000000000000000000000000000010"),
    ("attestationConfig", "0x0300000000000000000000000000000000000001"),
    ("babyjubjubConfig", "0x0500000000000000000000000000000000000007"),
    ("blake3Config", "0x0500000000000000000000000000000000000004"),
    ("bls12381G1AddConfig", "0x000000000000000000000000000000000000000b"),
    ("bls12381G1MSMConfig", "0x000000000000000000000000000000000000000d"),
    ("bls12381G1MulConfig", "0x000000000000000000000000000000000000000c"),
    ("bls12381G2AddConfig", "0x000000000000000000000000000000000000000e"),
    ("bls12381G2MSMConfig", "0x0000000000000000000000000000000000000010"),
    ("bls12381G2MulConfig", "0x000000000000000000000000000000000000000f"),
    ("bls12381PairingConfig", "0x0000000000000000000000000000000000000011"),
    ("bridgeRegistrarConfig", "0x0400000000000000000000000000000000000046"),
    ("cggmp21Config", "0x0800000000000000000000000000000000000003"),
    ("computeMarketConfig", "0x0300000000000000000000000000000000000010"),
    ("contractDeployerAllowListConfig", "0x0000000000000000000000000000000000010201"),
    ("contractNativeMinterConfig", "0x0000000000000000000000000000000000010401"),
    ("coronaConfig", "0x0000000000000000000000000000000000012206"),
    ("curve25519Config", "0x0000000000000000000000000000000000009204"),
    ("deadConfig", "0x000000000000000000000000000000000000dead"),
    ("deadFullConfig", "0xdead000000000000000000000000000000000000"),
    ("deadZeroConfig", "0x0000000000000000000000000000000000000000"),
    ("dexPositionManagerConfig", "0x0000000000000000000000000000000000009996"),
    ("dexQuoterConfig", "0x0000000000000000000000000000000000009998"),
    ("dexSettleConfig", "0x0000000000000000000000000000000000009999"),
    ("dexStateViewConfig", "0x0000000000000000000000000000000000009997"),
    ("ed25519Config", "0x3211000000000000000000000000000000000000"),
    ("feeManagerConfig", "0x0000000000000000000000000000000000013f01"),
    ("fheConfig", "0x0700000000000000000000000000000000000000"),
    ("fixedPointMathConfig", "0x0400000000000000000000000000000000000050"),
    ("frostConfig", "0x0800000000000000000000000000000000000002"),
    ("graphConfig", "0x0500000000000000000000000000000000000010"),
    ("hpkeConfig", "0x0000000000000000000000000000000000009200"),
    ("hqcConfig", "0x0000000000000000000000000000000000012208"),
    ("kzg4844Config", "0x000000000000000000000000000000000000b002"),
    ("magnetarConfig", "0x0000000000000000000000000000000000012207"),
    ("mldsaConfig", "0x0000000000000000000000000000000000012202"),
    ("mlkemConfig", "0x0000000000000000000000000000000000012201"),
    ("modelRegistryConfig", "0x0300000000000000000000000000000000000002"),
    ("p3qConfig", "0x0000000000000000000000000000000000012205"),
    ("pastaConfig", "0x0500000000000000000000000000000000000008"),
    ("pedersenConfig", "0x0500000000000000000000000000000000000006"),
    ("poseidonConfig", "0x0500000000000000000000000000000000000005"),
    ("pulsarConfig", "0x0000000000000000000000000000000000012204"),
    ("rewardManagerConfig", "0x0000000000000000000000000000000000010205"),
    ("ringConfig", "0x0000000000000000000000000000000000009202"),
    ("routerConfig", "0x0000000000000000000000000000000000009012"),
    ("secp256r1Config", "0x0000000000000000000000000000000000000100"),
    ("slhdsaConfig", "0x0000000000000000000000000000000000012203"),
    ("sr25519Config", "0x0a00000000000000000000000000000000000001"),
    ("stableSwapConfig", "0x0400000000000000000000000000000000000060"),
    ("stakeWeightConfig", "0x0200000000000000000000000000000000000006"),
    ("starkfriConfig", "0x0000000000000000000000000000000000012220"),
    ("swapConfig", "0x00000000000000000000000000000000000090a0"),
    ("txAllowListConfig", "0x0000000000000000000000000000000000010301"),
    ("v3Config", "0x00000000000000000000000000000000000090a1"),
    ("vrfConfig", "0x0000000000000000000000000000000000003213"),
    ("warpConfig", "0x0200000000000000000000000000000000000005"),
    ("x25519Config", "0x0000000000000000000000000000000000009203"),
    ("xwingConfig", "0x0000000000000000000000000000000000002221"),
    ("zkConfig", "0x0900000000000000000000000000000000000000"),
];

/// The address a config key names, if luxd registers one.
pub fn address_of(key: &str) -> Option<Address> {
    REGISTRY.iter().find(|(k, _)| *k == key).map(|(_, a)| parse(a))
}

fn parse(hex: &str) -> Address {
    hex.parse::<Address>().expect("a registry address literal")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registry_is_luxds_and_the_addresses_parse() {
        assert_eq!(REGISTRY.len(), 63, "every module luxd registers");
        for (key, hex) in REGISTRY {
            let a = address_of(key).unwrap_or_else(|| panic!("{key} resolves"));
            assert_eq!(a, parse(hex));
        }
        // The one the allocation spells out by hand, so the two agree.
        assert_eq!(address_of("warpConfig").expect("warp"), lux_evm::rules::WARP);
    }
}
