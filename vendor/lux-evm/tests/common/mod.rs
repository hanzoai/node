// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// See the file LICENSE for licensing terms.

//! One transaction, run for real.
//!
//! Everything the integration tests check is read off a committed state, not
//! off a mock: the same handler, the same precompile set and the same
//! interpreter a node would use.
//!
//! Each test binary compiles its own copy of this module and uses part of it,
//! so the unused halves are expected rather than dead.
#![allow(dead_code)]

use revm::context::result::{EVMError, ExecutionResult};
use revm::context::{Context, ContextSetters, ContextTr, Evm, TxEnv};
use revm::context_interface::JournalTr;
use revm::database::{CacheDB, EmptyDB};
use revm::handler::instructions::EthInstructions;
use revm::handler::Handler as _;
use revm::primitives::{address, hardfork::SpecId, Address, Bytes, TxKind, U256};
use revm::state::{AccountInfo, Bytecode};
use revm::MainContext;

use lux_evm::{Chain, Handler, Precompiles, Profile};

/// Shanghai: `LuxMainnetChainConfig` sets `ShanghaiTime: 0` and leaves
/// `CancunTime` unset, and the C-chain genesis header carries no withdrawals
/// root — London's field set under Shanghai's rules.
pub const SPEC: SpecId = SpecId::SHANGHAI;

pub const CALLER: Address = address!("00000000000000000000000000000000000000a1");
pub const COINBASE: Address = address!("00000000000000000000000000000000000000c0");
pub const TARGET: Address = address!("00000000000000000000000000000000000000ff");

pub const BASEFEE: u64 = 20_000_000_000;
pub const PRICE: u128 = 25_000_000_000;
pub const LIMIT: u64 = 100_000;

/// What to run.
pub struct Run {
    pub chain: Chain,
    pub profile: Profile,
    pub code: &'static [u8],
    /// A storage slot to seed on the target before the call.
    pub slot: Option<(U256, U256)>,
    pub time: u64,
}

impl Default for Run {
    fn default() -> Self {
        Self {
            chain: Chain::lux(),
            profile: Profile::default(),
            code: &[],
            slot: None,
            time: 0,
        }
    }
}

/// What the block observed.
pub struct Outcome {
    pub gas_used: u64,
    pub coinbase: U256,
    /// What the sender paid, all in: the balance it no longer has.
    pub spent: U256,
    pub output: Bytes,
}

/// The sender's opening balance, so a debit can be read as a difference.
pub fn funded() -> U256 {
    U256::from(10u64).pow(U256::from(20u64))
}

type Error = EVMError<core::convert::Infallible>;

pub fn run(r: Run) -> Outcome {
    let mut db = CacheDB::new(EmptyDB::default());
    db.insert_account_info(
        CALLER,
        AccountInfo {
            balance: funded(),
            ..Default::default()
        },
    );
    if !r.code.is_empty() {
        db.insert_account_info(
            TARGET,
            AccountInfo::default().with_code(Bytecode::new_raw(Bytes::copy_from_slice(r.code))),
        );
    }
    if let Some((slot, value)) = r.slot {
        db.insert_account_storage(TARGET, slot, value).unwrap();
    }

    let ctx = Context::mainnet()
        .with_db(db)
        .modify_cfg_chained(|c| c.spec = SPEC)
        .modify_block_chained(|b| {
            b.beneficiary = COINBASE;
            b.basefee = BASEFEE;
            b.timestamp = U256::from(r.time);
        });
    let mut evm = Evm::new(
        ctx,
        EthInstructions::new_mainnet_with_spec(SPEC),
        Precompiles::new(SPEC, r.profile),
    );

    evm.ctx.set_tx(
        TxEnv::builder()
            .caller(CALLER)
            .kind(TxKind::Call(TARGET))
            .gas_limit(LIMIT)
            .gas_price(PRICE)
            .nonce(0)
            .build_fill(),
    );

    let mut handler: Handler<_, Error, _> = Handler::new(r.chain);
    let result: ExecutionResult = handler.run(&mut evm).expect("transaction ran");
    assert!(result.is_success(), "unexpected outcome: {result:?}");

    let state = evm.ctx.journal_mut().finalize();
    let balance = |who| {
        state
            .get(&who)
            .map(|a: &revm::state::Account| a.info.balance)
            .unwrap_or_default()
    };
    Outcome {
        gas_used: result.gas_used(),
        coinbase: balance(COINBASE),
        spent: funded() - balance(CALLER),
        output: result.into_output().unwrap_or_default(),
    }
}
