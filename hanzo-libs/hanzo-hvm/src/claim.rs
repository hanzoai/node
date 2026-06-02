//! A-Chain proof → mint (HIP-0001 settlement / HIP-0096 rewards).
//!
//! A `ComputeProof` is committed on the Lux primary-network **A-Chain**
//! (aivm input/output commitments + NVTrust attestation). To get paid, a
//! provider presents the proof and **mints $AI on the chain of their choice**.
//! The proof's commitment triple is the **nullifier**: it is consumed once at
//! the root, so a claim cannot be double-spent across chains.
use std::collections::HashSet;

/// Verified compute committed on the A-Chain (the shared proof root).
#[derive(Clone, Debug)]
pub struct ComputeProof {
    pub job_id: [u8; 32],
    pub input_commitment: [u8; 32],  // from aivm inference_commit
    pub output_commitment: [u8; 32], // from aivm inference_commit
    pub hcu: u64,                    // verified Hanzo Compute Units (HIP-0096 §3.1)
    pub attested: bool,              // NVTrust GPU-TEE verified ⇒ trust premium
    pub provider: [u8; 20],          // EVM address to credit
}

impl ComputeProof {
    /// Nullifier = the on-chain commitment triple (job, input, output).
    fn nullifier(&self) -> ([u8; 32], [u8; 32], [u8; 32]) {
        (self.job_id, self.input_commitment, self.output_commitment)
    }
}

/// A minted claim, redeemable on any natively-supported chain via Warp/Teleport.
#[derive(Clone, Debug, PartialEq)]
pub struct Mint {
    pub to: [u8; 20],
    pub amount_ai_wei: u128,
    pub target_chain_id: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ClaimError {
    DoubleClaim,
    ZeroWork,
}

/// The A-Chain claim ledger: the single, non-duplicable source of claim truth.
#[derive(Default)]
pub struct AChainMintLedger {
    spent: HashSet<([u8; 32], [u8; 32], [u8; 32])>,
}

impl AChainMintLedger {
    /// Mint $AI for a proof. `price_ai_wei_per_hcu` comes from the HVM compute
    /// DEX (market-priced, not hardcoded). Attested (NVTrust) work earns a +8%
    /// confidential-compute premium (HIP-0096). Idempotent per proof.
    pub fn claim(&mut self, proof: &ComputeProof, price_ai_wei_per_hcu: u128, target_chain_id: u64) -> Result<Mint, ClaimError> {
        if proof.hcu == 0 {
            return Err(ClaimError::ZeroWork);
        }
        let n = proof.nullifier();
        if self.spent.contains(&n) {
            return Err(ClaimError::DoubleClaim);
        }
        let mut amount = proof.hcu as u128 * price_ai_wei_per_hcu;
        if proof.attested {
            amount += amount * 8 / 100; // verified GPU-TEE trust premium
        }
        self.spent.insert(n);
        Ok(Mint { to: proof.provider, amount_ai_wei: amount, target_chain_id })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn proof(attested: bool) -> ComputeProof {
        ComputeProof { job_id: [1; 32], input_commitment: [2; 32], output_commitment: [3; 32], hcu: 1000, attested, provider: [0xAB; 20] }
    }

    #[test]
    fn mints_priced_by_market_with_trust_premium() {
        let mut led = AChainMintLedger::default();
        let m = led.claim(&proof(false), 1_000_000_000, 1).unwrap(); // 1e9 wei/HCU
        assert_eq!(m.amount_ai_wei, 1_000 * 1_000_000_000); // hcu * price
        let mut led2 = AChainMintLedger::default();
        let m2 = led2.claim(&proof(true), 1_000_000_000, 1).unwrap();
        assert_eq!(m2.amount_ai_wei, 1_000 * 1_000_000_000 * 108 / 100); // +8% attested
    }

    #[test]
    fn proof_consumed_once_no_double_claim() {
        let mut led = AChainMintLedger::default();
        assert!(led.claim(&proof(false), 1_000_000_000, 1).is_ok());
        // same proof, even targeting a different chain, is rejected
        assert_eq!(led.claim(&proof(false), 1_000_000_000, 8453).unwrap_err(), ClaimError::DoubleClaim);
    }

    #[test]
    fn zero_work_rejected() {
        let mut led = AChainMintLedger::default();
        let mut p = proof(false); p.hcu = 0;
        assert_eq!(led.claim(&p, 1_000_000_000, 1).unwrap_err(), ClaimError::ZeroWork);
    }
}
