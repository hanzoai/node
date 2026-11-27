// SPDX-License-Identifier: BSD-3-Clause-Eco
// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// See the file LICENSE for licensing terms.

//! Reading a chain back out of its RLP export.
//!
//! An export is the C-chain's history in the one format it has outside itself:
//! a bare concatenation of RLP-encoded blocks, block zero first, no envelope
//! and no index. Go reads it with `--import-chain-data`, which is the flag this
//! module exists to answer; the name and the meaning are the same on purpose,
//! so one runbook works for every runtime.
//!
//! NOTHING IS TAKEN ON TRUST. The export carries a header's claims — a state
//! root, a receipts root, a gas total — and this module hands every block to
//! [`Vm::verify`], which re-executes it and refuses a header whose claims are
//! not what execution produced. Then the link: a block's parent must be the
//! block that came before it, checked hash by hash rather than believed because
//! the file said so. An import that skipped either would be a way to hand a
//! node any history at all.
//!
//! WHAT AN IMPORT DOES NOT DO IS AGREE. Every block here arrives with no
//! certificate: this node executed them, and no quorum said anything about
//! them. Go records the same wound from the other side — its proposervm finds
//! an inner chain whose outer envelopes are absent and says the chain "MUST NOT
//! be treated as a caught-up validator until the outer index is rebuilt from
//! certified peer state" (`vms/proposervm/vm.go`). So the import CLOSES A
//! [`Gate`] before it accepts its first block — on the device, in the data
//! directory, so a restart without the flag finds it — and a node behind that
//! gate will not build and will not sign. A runtime that imported and then
//! proposed would be signing history it never saw agreed to, which is worse
//! than one that cannot import at all.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use crate::gate::Gate;
use crate::vm::{Id, Vm};

/// How many blocks go between two syncs. The history an import accepts is on
/// the device at each of these and at the end; a crash between two costs the
/// blocks since the last, read again from the export on the next run.
const SYNC_EVERY: usize = 4096;

/// The largest RLP item this will read.
///
/// A length prefix is eight bytes of attacker-chosen number, and a reader that
/// allocated what it was told to would be a file away from an out-of-memory
/// kill. No C-chain block is anywhere near this.
const LARGEST: usize = 32 << 20;

/// What an import left behind.
///
/// `certified` is deliberately absent: an import produces none, and a field
/// that could say otherwise would be a place to write the lie.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Imported {
    /// How many blocks this import accepted. Zero when the chain was already
    /// at or above the export's tip — a re-run, not a failure.
    pub blocks: usize,
    /// The first height this import accepted, and so the first height with no
    /// certificate under it. Zero when nothing was accepted.
    pub from: u64,
    /// The accepted tip after the import.
    pub height: u64,
    pub tip: Id,
    /// The root of the world that tip leaves behind — recomputed by execution,
    /// never copied out of the header.
    pub state_root: Id,
}

/// Import the export at `path` into `vm`, behind `gate`.
pub fn from_file(vm: &dyn Vm, path: &Path, gate: &Gate) -> Result<Imported, String> {
    let file = File::open(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    from_reader(vm, BufReader::new(file), gate).map_err(|e| format!("{}: {e}", path.display()))
}

/// Import an export read from `source`, closing `gate` over every block it
/// accepts before that block is accepted.
///
/// Streamed, one block at a time: a mainnet export is gigabytes and a reader
/// that held it all would need more memory than the chain it is importing.
///
/// Each block is accepted as history ([`Vm::accept_history`]) and the chain
/// synced every [`SYNC_EVERY`] blocks and before this returns, failed or not:
/// nothing is built, signed or served on an imported block before it is on
/// the device, which is what waiting at every block would buy, at a sync a
/// block.
pub fn from_reader(vm: &dyn Vm, source: impl Read, gate: &Gate) -> Result<Imported, String> {
    let read = import(vm, source, gate);
    let synced = vm.sync().map_err(|e| format!("the imported chain did not reach the disk: {e}"));
    let done = read?;
    synced?;
    Ok(done)
}

fn import(vm: &dyn Vm, mut source: impl Read, gate: &Gate) -> Result<Imported, String> {
    let mut tip = vm.last_accepted();
    let mut height = vm
        .get(&tip)
        .map(|b| b.height())
        .map_err(|e| format!("the chain has no accepted block to import onto: {e}"))?;
    let mut state_root = vm.get(&tip).map(|b| b.state_root()).unwrap_or_default();
    let head = height;

    let mut blocks = 0usize;
    let mut from = 0u64;
    let mut seen = 0usize;

    while let Some(raw) = item(&mut source)? {
        seen += 1;
        let block = vm
            .parse(&raw)
            .map_err(|e| format!("block {seen} of the export is not a block: {e}"))?;
        let at = block.height();

        // Block zero is the genesis, which the chain already has — and a chain
        // whose genesis differs from the export's is a different chain, which
        // the first link check below is what catches.
        if at == 0 {
            continue;
        }
        // Already here. Go skips these too, which is what makes a second run of
        // the same flag a no-op rather than an error.
        if at <= head {
            continue;
        }

        // THE LINK. Not "the file is in order" but "this block's parent is the
        // block this node just accepted", hash against hash. It is the one
        // check that cannot be satisfied by a well-formed export of a different
        // chain.
        if block.parent() != tip {
            return Err(format!(
                "block {at} names parent {} — the accepted chain is at {} ({})",
                hex(&block.parent()),
                height,
                hex(&tip),
            ));
        }
        if at != height + 1 {
            return Err(format!("block {at} follows height {height}"));
        }

        let id = block.id();
        // Execute it. `verify` recomputes the state root, the receipts root,
        // the transactions root and the gas, and refuses the block if any of
        // them is not what the header claims.
        vm.verify(&id).map_err(|e| format!("block {at} does not verify: {e}"))?;
        if blocks == 0 {
            from = at;
        }
        // THE GATE BEFORE THE BLOCK. Once this block is accepted the node
        // holds history no quorum spoke about, so the record that it does is
        // on the device first; a crash between the two leaves a gate over a
        // block never taken, which costs a wait, never a signature.
        gate.hold(from, at).map_err(|e| format!("block {at}: the import gate: {e}"))?;
        vm.accept_history(&id).map_err(|e| format!("block {at} was not accepted: {e}"))?;
        vm.set_preference(&id).map_err(|e| format!("block {at} could not be preferred: {e}"))?;

        blocks += 1;
        if blocks.is_multiple_of(SYNC_EVERY) {
            vm.sync().map_err(|e| format!("block {at} did not reach the disk: {e}"))?;
        }
        height = at;
        tip = id;
        state_root = block.state_root();
    }

    if seen == 0 {
        return Err("there are no blocks in this export".into());
    }
    Ok(Imported { blocks, from, height, tip, state_root })
}

/// The next RLP item in `source`, or `None` at a clean end of the export.
///
/// Reading the length prefix rather than the whole file is what makes this
/// streamable, and the bytes returned are the item's own encoding — exactly
/// what [`Vm::parse`] takes and what a peer would have been sent.
fn item(source: &mut impl Read) -> Result<Option<Vec<u8>>, String> {
    let mut tag = [0u8; 1];
    match source.read(&mut tag) {
        Ok(0) => return Ok(None),
        Ok(_) => {}
        Err(e) => return Err(format!("cannot read: {e}")),
    }
    let tag = tag[0];
    if tag < 0xc0 {
        return Err(format!("an export holds blocks; found a {tag:#04x} item"));
    }

    let mut out = vec![tag];
    let payload = if tag <= 0xf7 {
        (tag - 0xc0) as usize
    } else {
        let n = (tag - 0xf7) as usize;
        let mut be = vec![0u8; n];
        read_exact(source, &mut be)?;
        if be[0] == 0 {
            return Err("a length with a leading zero is not canonical RLP".into());
        }
        // Accumulated wide, so a length that does not fit is REFUSED rather
        // than wrapped into a small one — the wrap is how a huge declared item
        // becomes a plausible short read.
        let mut len = 0u128;
        for byte in &be {
            len = (len << 8) | *byte as u128;
        }
        out.extend_from_slice(&be);
        if len > LARGEST as u128 {
            return Err(format!("a {len}-byte item is larger than any block"));
        }
        len as usize
    };
    if payload > LARGEST {
        return Err(format!("a {payload}-byte item is larger than any block"));
    }
    let at = out.len();
    out.resize(at + payload, 0);
    read_exact(source, &mut out[at..])?;
    Ok(Some(out))
}

fn read_exact(source: &mut impl Read, into: &mut [u8]) -> Result<(), String> {
    source.read_exact(into).map_err(|e| format!("the export ends inside a block: {e}"))
}

fn hex(id: &Id) -> String {
    alloy_primitives::B256::from(*id).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn a_short_list_is_read_whole() {
        // 0xc3 830102 03 — a three-byte list, and the bytes come back with the
        // prefix, because that is what a decoder is handed.
        let mut r = Cursor::new(vec![0xc3, 0x01, 0x02, 0x03, 0xc1, 0x04]);
        assert_eq!(item(&mut r).unwrap().unwrap(), vec![0xc3, 0x01, 0x02, 0x03]);
        assert_eq!(item(&mut r).unwrap().unwrap(), vec![0xc1, 0x04]);
        assert_eq!(item(&mut r).unwrap(), None);
    }

    #[test]
    fn a_long_list_reads_its_length_first() {
        let mut body = vec![0xf8, 0x40];
        body.extend(std::iter::repeat_n(0x11, 0x40));
        let mut r = Cursor::new(body.clone());
        assert_eq!(item(&mut r).unwrap().unwrap(), body);
    }

    #[test]
    fn a_declared_length_is_not_believed() {
        // 0xfb + seven length bytes claiming ~2^55 bytes. A reader that
        // allocated first would be killed by a file.
        let mut r = Cursor::new(vec![0xfb, 0x7f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
        let why = item(&mut r).unwrap_err();
        assert!(why.contains("larger than any block"), "{why}");
    }

    #[test]
    fn a_truncated_export_is_an_error_not_an_end() {
        let mut r = Cursor::new(vec![0xc3, 0x01]);
        assert!(item(&mut r).unwrap_err().contains("ends inside a block"));
    }

    #[test]
    fn a_leading_zero_length_is_refused() {
        let mut r = Cursor::new(vec![0xf8, 0x00]);
        assert!(item(&mut r).unwrap_err().contains("canonical"));
    }

    #[test]
    fn a_non_list_is_not_a_block() {
        let mut r = Cursor::new(vec![0x83, 0x01, 0x02, 0x03]);
        assert!(item(&mut r).unwrap_err().contains("holds blocks"));
    }

    // The round trip, on a chain this process produced: the export a node
    // writes is the export another node reads, and the second arrives at the
    // first's tip by executing, not by copying.
    use crate::evm::tests_support::{raw_transfer, who};
    use crate::evm::Evm;
    use alloy_primitives::U256;
    use lux_evm::Genesis;

    fn chain() -> Evm {
        Evm::new(Genesis {
            alloc: vec![(who(7), lux_evm::Alloc::funded(U256::from(10u64).pow(U256::from(21u64))))],
            fee: crate::evm::tests_support::UNMETERED,
            ..lux_evm::genesis::localnet()
        })
    }

    /// `n` blocks, and the bytes an export of them would hold.
    fn exported(n: u64) -> (Evm, Vec<u8>, Id, Id) {
        let source = chain();
        let mut out = source.get(&source.last_accepted()).expect("genesis").bytes();
        for nonce in 0..n {
            source
                .submit(&raw_transfer(nonce, source.chain_id(), who(9), U256::from(7u64)))
                .expect("submitted");
            let block = source.build().expect("built");
            let id = block.id();
            source.verify(&id).expect("verifies");
            source.accept(&id).expect("accepts");
            out.extend_from_slice(&block.bytes());
        }
        let tip = source.last_accepted();
        let root = source.get(&tip).expect("tip").state_root();
        (source, out, tip, root)
    }

    #[test]
    fn an_export_replays_to_the_same_tip_and_the_same_root() {
        let (_source, bytes, tip, root) = exported(4);
        let onto = chain();
        let gate = Gate::memory();
        let done = from_reader(&onto, Cursor::new(bytes), &gate).expect("imports");
        assert_eq!(gate.held(), Some((1, 4)), "every height it took is behind the gate");
        assert_eq!(done.blocks, 4);
        assert_eq!(done.from, 1);
        assert_eq!(done.height, 4);
        assert_eq!(done.tip, tip);
        // The root is what THIS node's interpreter arrived at, and it is the
        // exporting node's. A copied header would pass a weaker test.
        assert_eq!(done.state_root, root);
        assert_eq!(onto.last_accepted(), tip);
    }

    #[test]
    fn a_second_import_of_the_same_export_changes_nothing() {
        // Go keeps the flag set across restarts, so this path runs on every
        // boot of an already-imported node. It is a no-op, not a failure.
        let (_source, bytes, tip, _) = exported(3);
        let onto = chain();
        from_reader(&onto, Cursor::new(bytes.clone()), &Gate::memory()).expect("imports");
        // A node the peers already anchored, restarted with the flag still
        // set: nothing is taken, so nothing closes the gate again.
        let anchored = Gate::memory();
        let again = from_reader(&onto, Cursor::new(bytes), &anchored).expect("re-runs");
        assert_eq!(again.blocks, 0);
        assert_eq!(again.height, 3);
        assert_eq!(again.tip, tip);
        assert_eq!(anchored.held(), None);
    }

    #[test]
    fn no_block_is_accepted_before_the_gate_is_on_the_device() {
        // A gate that cannot be written. If the import accepted first and
        // closed the gate after, block one would be in; it must not be.
        let (_source, bytes, _, _) = exported(2);
        let onto = chain();
        let gate = Gate::open("/nonexistent/lux-gate/uncertified", 0).expect("absent is open");
        let why = from_reader(&onto, Cursor::new(bytes), &gate).unwrap_err();
        assert!(why.contains("block 1: the import gate"), "{why}");
        assert_eq!(onto.height(), 0, "a block was accepted with no gate behind it");
    }

    #[test]
    fn a_block_from_another_chain_does_not_link() {
        // The check that makes an export something other than "whatever the
        // file said": block one's parent must be THIS chain's genesis.
        let (_source, bytes, _, _) = exported(2);
        // A different genesis timestamp is a different genesis hash, and so a
        // different chain — the same shape of file, and not this history.
        let onto = Evm::new(Genesis {
            timestamp: lux_evm::genesis::localnet().timestamp + 1,
            alloc: vec![(who(7), lux_evm::Alloc::funded(U256::from(10u64).pow(U256::from(21u64))))],
            fee: crate::evm::tests_support::UNMETERED,
            ..lux_evm::genesis::localnet()
        });
        let why = from_reader(&onto, Cursor::new(bytes), &Gate::memory()).unwrap_err();
        assert!(why.contains("names parent"), "{why}");
    }

    #[test]
    fn a_block_whose_root_is_a_lie_is_refused_by_the_import_too() {
        // An import is not a back door into a state nobody executed.
        let (_source, bytes, _, _) = exported(1);
        let mut forged = bytes;
        // Flip a byte inside the last block's payload. Whatever it lands on —
        // a root, a gas total, a signature — the block no longer describes what
        // running it produces.
        let at = forged.len() - 40;
        forged[at] ^= 0xff;
        let onto = chain();
        let gate = Gate::memory();
        assert!(from_reader(&onto, Cursor::new(forged), &gate).is_err());
        assert_eq!(gate.held(), None, "nothing was taken, so nothing is gated");
    }

    #[test]
    fn an_empty_export_is_an_error() {
        let onto = chain();
        let empty = from_reader(&onto, Cursor::new(Vec::new()), &Gate::memory());
        assert!(empty.unwrap_err().contains("no blocks"));
    }
}
