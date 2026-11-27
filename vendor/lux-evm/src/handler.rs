// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// See the file LICENSE for licensing terms.

//! Where the Lux rules meet revm.
//!
//! revm's [`revm::handler::Handler`] is a trait with defaults, so a chain that
//! differs from Ethereum in two places overrides two methods and inherits
//! everything else. That is the whole surface: [`Handler::refund`] decides what
//! the sender gets back, [`Handler::reward_beneficiary`] decides what the
//! miner receives, and [`crate::rules`] answers both.
//!
//! Nothing else about execution is touched. An interpreter fork would be a
//! second interpreter to keep in step with Go and C++; this is a policy.

use core::marker::PhantomData;

use revm::context::journaled_state::account::JournaledAccountTr;
use revm::context::{Block, Cfg, ContextTr, JournalTr, Transaction};
use revm::context_interface::result::HaltReason;
use revm::handler::{
    post_execution, EvmTr, EvmTrError, FrameResult, FrameTr, Handler as HandlerTr,
};
use revm::interpreter::interpreter_action::FrameInit;
use revm::interpreter::Gas;
use revm::primitives::U256;
use revm::state::EvmState;
use revm::Database;

use crate::rules::{Chain, Rules};

/// revm's mainnet handler with the Lux rules in place of Ethereum's two.
#[derive(Clone, Debug)]
pub struct Handler<EVM, ERROR, FRAME> {
    /// When each Lux rule turns on.
    pub chain: Chain,
    phantom: PhantomData<(EVM, ERROR, FRAME)>,
}

impl<EVM, ERROR, FRAME> Handler<EVM, ERROR, FRAME> {
    /// A handler for `chain`.
    pub const fn new(chain: Chain) -> Self {
        Self {
            chain,
            phantom: PhantomData,
        }
    }
}

impl<EVM, ERROR, FRAME> Default for Handler<EVM, ERROR, FRAME> {
    /// The C-chain's rules, because that is the chain this crate exists for.
    fn default() -> Self {
        Self::new(Chain::lux())
    }
}

impl<EVM, ERROR, FRAME> HandlerTr for Handler<EVM, ERROR, FRAME>
where
    EVM: EvmTr<Context: ContextTr<Journal: JournalTr<State = EvmState>>, Frame = FRAME>,
    ERROR: EvmTrError<EVM>,
    FRAME: FrameTr<FrameResult = FrameResult, FrameInit = FrameInit>,
{
    type Evm = EVM;
    type Error = ERROR;
    type HaltReason = HaltReason;

    fn refund(
        &self,
        evm: &mut Self::Evm,
        exec_result: &mut <<Self::Evm as EvmTr>::Frame as FrameTr>::FrameResult,
        eip7702_refund: i64,
    ) {
        if self.rules(evm).refunds() {
            let spec = evm.ctx().cfg().spec().into();
            post_execution::refund(spec, exec_result.gas_mut(), eip7702_refund);
            return;
        }
        // Lux: the counter is discarded, so gasUsed is gasSpent. Zeroing here
        // rather than skipping the call is deliberate — an EIP-7702 refund is
        // recorded on the gas before this method runs, and leaving it in place
        // would refund under a rule that says there are no refunds.
        //
        // This is the whole change. revm reimburses the sender
        // `(remaining + refunded) * price` and computes `used` as
        // `spent - refunded`, so a zeroed counter is at once the sender getting
        // nothing back and the block reporting the larger gasUsed — which is
        // exactly the two halves of Go's `refundGas` under `isEVM`.
        exec_result.gas_mut().set_refund(0);
    }

    fn reward_beneficiary(
        &self,
        evm: &mut Self::Evm,
        exec_result: &mut <<Self::Evm as EvmTr>::Frame as FrameTr>::FrameResult,
    ) -> Result<(), Self::Error> {
        let rules = self.rules(evm);
        if !rules.evm {
            return post_execution::reward_beneficiary(evm.ctx(), exec_result.gas())
                .map_err(From::from);
        }
        reward(evm.ctx(), exec_result.gas(), rules).map_err(From::from)
    }
}

impl<EVM, ERROR, FRAME> Handler<EVM, ERROR, FRAME>
where
    EVM: EvmTr,
{
    /// The rules in force in the block being executed.
    fn rules(&self, evm: &mut EVM) -> Rules {
        let time = evm.ctx().block().timestamp();
        self.chain.at(time.saturating_to::<u64>())
    }
}

/// Credits the beneficiary under `rules`.
///
/// A free function rather than a method so the database error type stays
/// nameable; the handler maps it into its own error at the call site, which is
/// exactly what revm's own `reward_beneficiary` does.
fn reward<CTX: ContextTr>(
    context: &mut CTX,
    gas: &Gas,
    rules: Rules,
) -> Result<(), <CTX::Db as Database>::Error> {
    let used = gas.used();
    let (block, tx, _cfg, journal, _, _) = context.all_mut();
    let basefee = block.basefee() as u128;
    let price = tx.effective_gas_price(basefee);
    let beneficiary = block.beneficiary();

    journal
        .load_account_mut(beneficiary)?
        .incr_balance(U256::from(rules.reward(used, price, basefee)));
    Ok(())
}
