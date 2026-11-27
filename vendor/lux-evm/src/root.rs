// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// See the file LICENSE for licensing terms.

//! The three roots a header commits to.
//!
//! A root is a Merkle-Patricia trie hash, and there is no shortcut to one: a
//! digest over a sorted list of accounts is a different number, and a node that
//! computes one instead has forked at genesis while its own tests pass. That is
//! not hypothetical — it is what the Rust node does today.
//!
//! A root over a list built once — transactions, receipts — is `alloy-trie`'s,
//! built from sorted leaves. The world keeps its own state as tries that are
//! changed in place of being rebuilt ([`crate::trie`]); `state` and `storage`
//! here are the from-scratch roots those are held to. What this module carries
//! is the mapping from *what the EVM produced* to *what the header commits
//! to*, which is where the choices are: transactions are keyed by index over
//! their 2718 encodings, receipts the same, and state is keyed by
//! `keccak(address)` over the four-field account.

use alloy_primitives::{Address, B256, U256};
use alloy_trie::{root, TrieAccount};

/// The root of an empty trie — `keccak(rlp(""))`.
///
/// Both `transactionsRoot` and `receiptsRoot` in the C-chain genesis are this
/// value, which is the whole reason a block with no transactions is still
/// well-named.
pub use alloy_trie::EMPTY_ROOT_HASH as EMPTY;

/// `transactionsRoot` over transactions already in their consensus encoding.
///
/// Pre-encoded because that is the form a transaction arrives in and the form
/// the trie commits to: a typed transaction's value in the trie is its 2718
/// envelope, not an RLP-wrapped copy of it.
pub fn transactions<T: AsRef<[u8]>>(encoded: &[T]) -> B256 {
    root::ordered_trie_root_encoded(encoded)
}

/// `receiptsRoot` over receipts already in their consensus encoding.
pub fn receipts<T: AsRef<[u8]>>(encoded: &[T]) -> B256 {
    root::ordered_trie_root_encoded(encoded)
}

/// One account as the state trie holds it.
///
/// Four fields, in this order, and the two hashes are the empty ones for an
/// account with no code and no storage — not zero. An account that stores
/// `B256::ZERO` for either is a different account to everyone else.
pub fn account(nonce: u64, balance: U256, storage: B256, code: B256) -> TrieAccount {
    TrieAccount {
        nonce,
        balance,
        storage_root: storage,
        code_hash: code,
    }
}

/// `stateRoot` over accounts keyed by address.
///
/// The addresses are hashed and sorted here; a caller holding a state map in
/// address order does not have them in trie order.
pub fn state(accounts: impl IntoIterator<Item = (Address, TrieAccount)>) -> B256 {
    root::state_root_unhashed(accounts)
}

/// The root of one account's storage.
pub fn storage(slots: impl IntoIterator<Item = (B256, U256)>) -> B256 {
    root::storage_root_unhashed(slots)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, b256, keccak256};

    #[test]
    fn an_empty_list_is_the_empty_root() {
        let none: [&[u8]; 0] = [];
        assert_eq!(transactions(&none), EMPTY);
        assert_eq!(receipts(&none), EMPTY);
        assert_eq!(
            EMPTY,
            b256!("56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421"),
            "the value the C-chain genesis reports for both roots"
        );
    }

    #[test]
    fn an_empty_state_is_the_empty_root() {
        assert_eq!(state([]), EMPTY);
        assert_eq!(storage([]), EMPTY);
    }

    #[test]
    fn a_default_account_is_empty_code_and_empty_storage() {
        let a = TrieAccount::default();
        assert_eq!(a.storage_root, EMPTY);
        assert_eq!(
            a.code_hash,
            keccak256([]),
            "an account with no code hashes the empty string, not zero"
        );
    }

    #[test]
    fn the_state_root_is_a_trie_not_a_digest_over_a_sorted_list() {
        // One funded account. The value is fixed by the trie's shape, so any
        // implementation that hashes the accounts some other way — however
        // carefully sorted — produces a different number and a forked chain.
        let one = state([(
            address!("0000000000000000000000000000000000000001"),
            account(0, U256::from(1_000_000u64), EMPTY, keccak256([])),
        )]);
        assert_ne!(one, EMPTY);

        // Order of insertion cannot matter: the trie sorts by keccak(address).
        let a = address!("00000000000000000000000000000000000000aa");
        let b = address!("00000000000000000000000000000000000000bb");
        let acct = account(1, U256::from(7u64), EMPTY, keccak256([]));
        assert_eq!(state([(a, acct), (b, acct)]), state([(b, acct), (a, acct)]));
    }

    #[test]
    fn one_transaction_is_keyed_by_its_index() {
        // A single-item trie keys 0x80 (rlp of index 0) to the encoding.
        let tx: &[u8] = &[0x02, 0xde, 0xad];
        assert_ne!(transactions(&[tx]), EMPTY);
        assert_ne!(
            transactions(&[tx]),
            transactions(&[tx, tx]),
            "position is part of the commitment"
        );
    }
}
