// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// See the file LICENSE for licensing terms.

//! What a block is called.
//!
//! A block hash is not a field a node reports; it is keccak256 of the RLP of
//! the header, and two implementations agree on it only if they agree on which
//! fields exist and in what order. A Rust node that hashes a serialised struct
//! — any struct, however faithful its contents — has invented a second name for
//! every block on the chain.
//!
//! A LUX HEADER IS FIFTEEN ETHEREUM FIELDS AND A TAIL WHOSE MEANING DEPENDS ON
//! ITS LENGTH AND ON WHAT EACH POSITION HOLDS. coreth wrote `ExtDataHash` at
//! position 15; Lux geth writes the base fee there; Ethereum's Shanghai and
//! Cancun shapes share the same field counts. `luxfi/geth`'s
//! `core/types.DecodeHeader` (decode.go) picks the candidate shapes by field
//! count and tries them in a fixed order, the first that reads every position
//! winning. That order IS the decoder, so it is written down here as data —
//! [`LAYOUTS`] — and a header remembers the [`Layout`] it was read in, so it
//! re-encodes to exactly the bytes it arrived as.
//!
//! Every Lux, Zoo and SPC block on record is `[baseFee, ""]`: seventeen fields,
//! the seventeenth an ABSENT `ExtDataHash`. Go reads no block gas cost there,
//! and a block that carries one is written in the nineteen-field shape Go's
//! `EncodeRLP19Lux` writes.
//!
//! `luxfi/conformance`'s `evm_header_layouts.json` is Go's own answer on every
//! shape, and `tests/headers.rs` holds this module to it.

use alloy_consensus::Header;
use alloy_primitives::{keccak256, Bytes, B256, U256};
use alloy_rlp::{Decodable, Encodable};

/// What one position after a header's fifteen Ethereum fields holds.
///
/// A hash is exactly 32 bytes (Go's `common.Hash`); an `OrNil` hash is 32
/// bytes or the empty string, which is absent (`*common.Hash` with
/// `rlp:"nil"`). An integer is canonical: no leading zero, a byte below 0x80
/// never wrapped. `BaseFee` is 64 bits, as revm's block environment is;
/// `BlockGasCost` 256; `ExtDataGasUsed` any width, because Go reads it as a
/// `*big.Int` and nothing it runs bounds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    BaseFee,
    BlockGasCost,
    ExtDataHash,
    ExtDataHashOrNil,
    ExtDataGasUsed,
    BlobGasUsed,
    ExcessBlobGas,
    ParentBeaconRoot,
    ParentBeaconRootOrNil,
    WithdrawalsRoot,
    WithdrawalsRootOrNil,
    RequestsHash,
    RequestsHashOrNil,
}

/// A header's shape: what each position after the fifteenth holds, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub name: &'static str,
    pub slots: &'static [Slot],
    /// A shape `DecodeHeader` tries, rather than one `DecodeBlock` turns a
    /// header into.
    decoded: bool,
}

use Slot::*;

const fn layout(name: &'static str, slots: &'static [Slot]) -> Layout {
    Layout {
        name,
        slots,
        decoded: true,
    }
}

/// Every shape `DecodeHeader` reads, in the order it tries them for one field
/// count — coreth's, with `ExtDataHash` at position 15, first; Lux's 20 and 21
/// before Ethereum's — and then the two `DecodeBlock` makes of a 17-field
/// header whose block carries a fourth item (`RLPFormat17Eth`).
pub const LAYOUTS: [Layout; 18] = [
    layout("15", &[]),
    layout("16", &[BaseFee]),
    layout("17 coreth", &[ExtDataHash, BaseFee]),
    layout("17", &[BaseFee, ExtDataHashOrNil]),
    layout("18 coreth", &[ExtDataHash, BaseFee, ExtDataGasUsed]),
    layout("18", &[BaseFee, ExtDataHashOrNil, ExtDataGasUsed]),
    layout(
        "19 coreth",
        &[ExtDataHash, BaseFee, ExtDataGasUsed, BlockGasCost],
    ),
    layout(
        "19 lux",
        &[BaseFee, ExtDataHash, ExtDataGasUsed, BlockGasCost],
    ),
    layout(
        "19",
        &[BaseFee, ExtDataHashOrNil, ExtDataGasUsed, BlockGasCost],
    ),
    layout(
        "20 lux",
        &[
            BaseFee,
            ExtDataHashOrNil,
            ExtDataGasUsed,
            BlockGasCost,
            BlobGasUsed,
        ],
    ),
    layout(
        "20 eth",
        &[
            BaseFee,
            WithdrawalsRoot,
            BlobGasUsed,
            ExcessBlobGas,
            ParentBeaconRoot,
        ],
    ),
    layout(
        "21 lux",
        &[
            BaseFee,
            ExtDataHashOrNil,
            ExtDataGasUsed,
            BlockGasCost,
            BlobGasUsed,
            ExcessBlobGas,
        ],
    ),
    layout(
        "21 eth",
        &[
            BaseFee,
            WithdrawalsRoot,
            BlobGasUsed,
            ExcessBlobGas,
            ParentBeaconRoot,
            RequestsHash,
        ],
    ),
    layout(
        "22",
        &[
            BaseFee,
            ExtDataHashOrNil,
            ExtDataGasUsed,
            BlockGasCost,
            BlobGasUsed,
            ExcessBlobGas,
            ParentBeaconRootOrNil,
        ],
    ),
    layout(
        "23",
        &[
            BaseFee,
            ExtDataHashOrNil,
            ExtDataGasUsed,
            BlockGasCost,
            BlobGasUsed,
            ExcessBlobGas,
            ParentBeaconRootOrNil,
            WithdrawalsRootOrNil,
        ],
    ),
    layout(
        "24",
        &[
            BaseFee,
            ExtDataHashOrNil,
            ExtDataGasUsed,
            BlockGasCost,
            BlobGasUsed,
            ExcessBlobGas,
            ParentBeaconRootOrNil,
            WithdrawalsRootOrNil,
            RequestsHashOrNil,
        ],
    ),
    Layout {
        name: "17 coreth, withdrawals",
        slots: &[WithdrawalsRoot, BaseFee],
        decoded: false,
    },
    Layout {
        name: "17, withdrawals",
        slots: &[BaseFee, WithdrawalsRootOrNil],
        decoded: false,
    },
];

/// The layout named `name`. Every name used in this crate is in the table.
pub fn named(name: &str) -> Layout {
    *LAYOUTS
        .iter()
        .find(|l| l.name == name)
        .expect("a layout in the table")
}

/// A C-chain header: Ethereum's fields, the Lux fields, and the shape they
/// were written in.
///
/// Every constructor here and [`Lux::read`] produce a header whose fields fit
/// its layout. A field set where the layout has no position for it would be
/// dropped on the way out and the block named for a header without it, so
/// [`Lux::encoded`] refuses to write one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lux {
    pub header: Header,
    pub layout: Layout,
    pub block_gas_cost: Option<U256>,
    pub ext_data_hash: Option<B256>,
    /// Big-endian with no leading zero; empty is zero.
    pub ext_data_gas_used: Option<Bytes>,
}

impl Lux {
    /// A header in the plain Ethereum shape its fields name: a genesis.
    ///
    /// Fifteen fields with no base fee, sixteen with one, and the Shanghai,
    /// Cancun and Prague tails — the shapes a genesis document can produce.
    pub fn genesis(header: Header) -> Self {
        let h = &header;
        let name = match (
            h.base_fee_per_gas.is_some(),
            h.withdrawals_root.is_some(),
            h.blob_gas_used.is_some() && h.excess_blob_gas.is_some(),
            h.parent_beacon_block_root.is_some(),
            h.requests_hash.is_some(),
        ) {
            (false, false, false, false, false) => "15",
            (true, false, false, false, false) => "16",
            (true, true, false, false, false) => "17, withdrawals",
            (true, true, true, true, false) => "20 eth",
            (true, true, true, true, true) => "21 eth",
            _ => panic!("no header shape Go reads carries this set of post-London fields"),
        };
        Self {
            header,
            layout: named(name),
            block_gas_cost: None,
            ext_data_hash: None,
            ext_data_gas_used: None,
        }
    }

    /// A block this chain builds, in the shape luxfi/geth `Header.EncodeRLP`
    /// writes for its fields (core/types/block.go:370-395):
    ///
    /// - before Shanghai, "17" — `[baseFee, ""]`, the shape of every block on
    ///   record — or, owing a block gas cost, "19 lux" (`EncodeRLP19Lux`);
    /// - Shanghai, a withdrawals root: Ethereum's 17 fields, "17,
    ///   withdrawals", in a block that carries its (empty) withdrawals;
    /// - Cancun, the blob and beacon-root fields too: Ethereum's "20 eth";
    /// - after Shanghai and owing a cost, Go's generated encoder
    ///   (gen_header_rlp.go): "23", every Lux field then every Ethereum one,
    ///   an absent one written as the empty string.
    pub fn block(header: Header, cost: U256) -> Self {
        let shanghai = header.withdrawals_root.is_some();
        let cancun = header.parent_beacon_block_root.is_some();
        let name = match (cost.is_zero(), shanghai, cancun) {
            (true, false, _) => "17",
            (false, false, _) => "19 lux",
            (true, true, false) => "17, withdrawals",
            (true, true, true) => "20 eth",
            (false, true, _) => "23",
        };
        let (block_gas_cost, ext_data_hash, ext_data_gas_used) = match name {
            "19 lux" => (Some(cost), Some(B256::ZERO), Some(Bytes::new())),
            "23" => (Some(cost), None, Some(Bytes::new())),
            _ => (None, None, None),
        };
        Self {
            header,
            layout: named(name),
            block_gas_cost,
            ext_data_hash,
            ext_data_gas_used,
        }
    }

    /// How many fields this header encodes to.
    pub fn fields(&self) -> usize {
        15 + self.layout.slots.len()
    }

    fn holds(&self, any: &[Slot]) -> bool {
        self.layout.slots.iter().any(|s| any.contains(s))
    }

    /// The RLP whose keccak256 the network accepted as this block's name.
    pub fn encoded(&self) -> Vec<u8> {
        let h = &self.header;
        for (present, at, what) in [
            (
                h.base_fee_per_gas.is_some(),
                &[BaseFee][..],
                "baseFeePerGas",
            ),
            (
                self.block_gas_cost.is_some(),
                &[BlockGasCost],
                "blockGasCost",
            ),
            (
                self.ext_data_hash.is_some(),
                &[ExtDataHash, ExtDataHashOrNil],
                "extDataHash",
            ),
            (
                self.ext_data_gas_used.is_some(),
                &[ExtDataGasUsed],
                "extDataGasUsed",
            ),
            (h.blob_gas_used.is_some(), &[BlobGasUsed], "blobGasUsed"),
            (
                h.excess_blob_gas.is_some(),
                &[ExcessBlobGas],
                "excessBlobGas",
            ),
            (
                h.parent_beacon_block_root.is_some(),
                &[ParentBeaconRoot, ParentBeaconRootOrNil],
                "parentBeaconRoot",
            ),
            (
                h.withdrawals_root.is_some(),
                &[WithdrawalsRoot, WithdrawalsRootOrNil],
                "withdrawalsRoot",
            ),
            (
                h.requests_hash.is_some(),
                &[RequestsHash, RequestsHashOrNil],
                "requestsHash",
            ),
            (
                h.block_access_list_hash.is_some(),
                &[],
                "blockAccessListHash",
            ),
            (h.slot_number.is_some(), &[], "slotNumber"),
        ] {
            assert!(
                !present || self.holds(at),
                "a {}-field header ({}) has no position for {what}",
                self.fields(),
                self.layout.name
            );
        }

        let mut body = Vec::with_capacity(640);
        h.parent_hash.encode(&mut body);
        h.ommers_hash.encode(&mut body);
        h.beneficiary.encode(&mut body);
        h.state_root.encode(&mut body);
        h.transactions_root.encode(&mut body);
        h.receipts_root.encode(&mut body);
        h.logs_bloom.encode(&mut body);
        h.difficulty.encode(&mut body);
        h.number.encode(&mut body);
        h.gas_limit.encode(&mut body);
        h.gas_used.encode(&mut body);
        h.timestamp.encode(&mut body);
        h.extra_data.encode(&mut body);
        h.mix_hash.encode(&mut body);
        h.nonce.encode(&mut body);
        // Go writes an absent pointer as the empty string and an absent value
        // hash as the zero hash (EncodeRLP19Lux).
        let empty = |out: &mut Vec<u8>| out.push(alloy_rlp::EMPTY_STRING_CODE);
        let value = |v: Option<B256>, out: &mut Vec<u8>| v.unwrap_or_default().encode(out);
        let nil = |v: Option<B256>, out: &mut Vec<u8>| match v {
            Some(v) => v.encode(out),
            None => empty(out),
        };
        let int = |v: Option<u64>, out: &mut Vec<u8>| v.unwrap_or_default().encode(out);
        for slot in self.layout.slots {
            match slot {
                BaseFee => int(h.base_fee_per_gas, &mut body),
                BlockGasCost => self.block_gas_cost.unwrap_or_default().encode(&mut body),
                ExtDataHash => value(self.ext_data_hash, &mut body),
                ExtDataHashOrNil => nil(self.ext_data_hash, &mut body),
                ExtDataGasUsed => match &self.ext_data_gas_used {
                    Some(b) => b.encode(&mut body),
                    None => empty(&mut body),
                },
                BlobGasUsed => int(h.blob_gas_used, &mut body),
                ExcessBlobGas => int(h.excess_blob_gas, &mut body),
                ParentBeaconRoot => value(h.parent_beacon_block_root, &mut body),
                ParentBeaconRootOrNil => nil(h.parent_beacon_block_root, &mut body),
                WithdrawalsRoot => value(h.withdrawals_root, &mut body),
                WithdrawalsRootOrNil => nil(h.withdrawals_root, &mut body),
                RequestsHash => value(h.requests_hash, &mut body),
                RequestsHashOrNil => nil(h.requests_hash, &mut body),
            }
        }
        let mut out = Vec::with_capacity(body.len() + 3);
        alloy_rlp::Header {
            list: true,
            payload_length: body.len(),
        }
        .encode(&mut out);
        out.extend_from_slice(&body);
        out
    }

    /// The block hash.
    pub fn seal(&self) -> B256 {
        keccak256(self.encoded())
    }

    /// Read a header as `DecodeHeader` reads it: its field count names the
    /// candidate shapes, and the first that reads every position is the one.
    /// A position of the wrong kind — a 5-byte base fee where a 32-byte hash
    /// must be — fails that shape, not the header.
    ///
    /// The header that comes back re-encodes to `raw`, so the name it seals to
    /// is keccak256 of the bytes that arrived — the name Go gives it.
    pub fn read(raw: &[u8]) -> Result<Self, alloy_rlp::Error> {
        let mut outer = raw;
        let head = alloy_rlp::Header::decode(&mut outer)?;
        if !head.list {
            return Err(alloy_rlp::Error::UnexpectedString);
        }
        if outer.len() != head.payload_length {
            return Err(alloy_rlp::Error::Custom("bytes after the header"));
        }
        let mut c = outer;
        let header = Header {
            parent_hash: Decodable::decode(&mut c)?,
            ommers_hash: Decodable::decode(&mut c)?,
            beneficiary: Decodable::decode(&mut c)?,
            state_root: Decodable::decode(&mut c)?,
            transactions_root: Decodable::decode(&mut c)?,
            receipts_root: Decodable::decode(&mut c)?,
            logs_bloom: Decodable::decode(&mut c)?,
            difficulty: Decodable::decode(&mut c)?,
            number: Decodable::decode(&mut c)?,
            gas_limit: Decodable::decode(&mut c)?,
            gas_used: Decodable::decode(&mut c)?,
            timestamp: Decodable::decode(&mut c)?,
            extra_data: Decodable::decode(&mut c)?,
            mix_hash: Decodable::decode(&mut c)?,
            nonce: Decodable::decode(&mut c)?,
            ..Default::default()
        };

        let mut rest = 0usize;
        let mut count = c;
        while !count.is_empty() {
            skip(&mut count)?;
            rest += 1;
        }
        let mut why = alloy_rlp::Error::Custom("a header field count Go does not read");
        for l in LAYOUTS
            .iter()
            .filter(|l| l.decoded && l.slots.len() == rest)
        {
            let mut tail = c;
            match Self::read_tail(header.clone(), *l, &mut tail) {
                Ok(lux) => {
                    // Canonical RLP has one encoding, and every reader here
                    // refuses the others; this holds the name to the bytes.
                    if lux.encoded() != raw {
                        return Err(alloy_rlp::Error::Custom(
                            "a header that does not re-encode to its bytes",
                        ));
                    }
                    return Ok(lux);
                }
                Err(e) => why = e,
            }
        }
        Err(why)
    }

    fn read_tail(header: Header, layout: Layout, c: &mut &[u8]) -> Result<Self, alloy_rlp::Error> {
        let mut lux = Self {
            header,
            layout,
            block_gas_cost: None,
            ext_data_hash: None,
            ext_data_gas_used: None,
        };
        let h = &mut lux.header;
        for slot in layout.slots {
            match slot {
                BaseFee => h.base_fee_per_gas = Some(u64::decode(c)?),
                BlockGasCost => lux.block_gas_cost = Some(U256::decode(c)?),
                ExtDataHash => lux.ext_data_hash = Some(B256::decode(c)?),
                ExtDataHashOrNil => lux.ext_data_hash = nil(c)?,
                ExtDataGasUsed => {
                    let b = Bytes::decode(c)?;
                    if b.first() == Some(&0) {
                        return Err(alloy_rlp::Error::LeadingZero);
                    }
                    lux.ext_data_gas_used = Some(b);
                }
                BlobGasUsed => h.blob_gas_used = Some(u64::decode(c)?),
                ExcessBlobGas => h.excess_blob_gas = Some(u64::decode(c)?),
                ParentBeaconRoot => h.parent_beacon_block_root = Some(B256::decode(c)?),
                ParentBeaconRootOrNil => h.parent_beacon_block_root = nil(c)?,
                WithdrawalsRoot => h.withdrawals_root = Some(B256::decode(c)?),
                WithdrawalsRootOrNil => h.withdrawals_root = nil(c)?,
                RequestsHash => h.requests_hash = Some(B256::decode(c)?),
                RequestsHashOrNil => h.requests_hash = nil(c)?,
            }
        }
        Ok(lux)
    }

    /// `DecodeBlock`'s reading of a header in a block that has a fourth item:
    /// a 17-field header's hash is the withdrawals root, not `ExtDataHash`.
    /// The bytes, and so the name, are the same.
    pub fn as_withdrawals(&mut self) {
        let Some(hash) = self.ext_data_hash else {
            return;
        };
        self.layout = match self.layout.name {
            "17" => named("17, withdrawals"),
            "17 coreth" => named("17 coreth, withdrawals"),
            _ => return,
        };
        self.header.withdrawals_root = Some(hash);
        self.ext_data_hash = None;
    }
}

/// The empty string, which is absent, or a 32-byte hash.
fn nil(c: &mut &[u8]) -> Result<Option<B256>, alloy_rlp::Error> {
    if c.first() == Some(&alloy_rlp::EMPTY_STRING_CODE) {
        *c = &c[1..];
        return Ok(None);
    }
    B256::decode(c).map(Some)
}

/// Step over one item, however it is encoded inside.
fn skip(c: &mut &[u8]) -> Result<(), alloy_rlp::Error> {
    let mut at = *c;
    let h = alloy_rlp::Header::decode(&mut at)?;
    *c = &at[h.payload_length..];
    Ok(())
}

/// A header's parent, for callers that hold only the header.
pub trait ParentOf {
    fn parent_canonical(&self) -> [u8; 32];
}

impl ParentOf for Header {
    fn parent_canonical(&self) -> [u8; 32] {
        self.parent_hash.0
    }
}

impl Encodable for Lux {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        out.put_slice(&self.encoded());
    }

    fn length(&self) -> usize {
        self.encoded().len()
    }
}

impl Decodable for Lux {
    fn decode(buf: &mut &[u8]) -> Result<Self, alloy_rlp::Error> {
        // Measure the list, decode it, and advance the caller past exactly it —
        // a block is a header followed by its transactions, so leaving the
        // cursor anywhere else loses the rest of the block.
        let mut peek = *buf;
        let head = alloy_rlp::Header::decode(&mut peek)?;
        if !head.list {
            return Err(alloy_rlp::Error::UnexpectedString);
        }
        let consumed = (buf.len() - peek.len()) + head.payload_length;
        let whole = buf.get(..consumed).ok_or(alloy_rlp::Error::InputTooShort)?;
        let out = Lux::read(whole)?;
        *buf = &buf[consumed..];
        Ok(out)
    }
}

/// The name of a header already in its encoded form.
///
/// A block that arrived on the wire is bytes, and re-encoding a decoded header
/// to name it is a chance to name it differently. Hashing what arrived cannot.
pub fn seal_bytes(rlp: &[u8]) -> B256 {
    keccak256(rlp)
}

/// The block hash of a header with no Lux field: keccak256 of its RLP.
///
/// Correct for a genesis and for the Ethereum shapes, and NOT for a block this
/// chain builds — use [`Lux::seal`], which names the shape.
pub fn seal(header: &Header) -> B256 {
    keccak256(alloy_rlp::encode(header))
}

/// Whether `header` has the shape the C-chain seals — London and no further.
///
/// Worth checking before trusting a hash: every field added after London is
/// optional in the encoding, so a header carrying one hashes differently while
/// still looking like a header.
pub fn is_london(header: &Header) -> bool {
    header.base_fee_per_gas.is_some()
        && header.withdrawals_root.is_none()
        && header.blob_gas_used.is_none()
        && header.excess_blob_gas.is_none()
        && header.parent_beacon_block_root.is_none()
        && header.requests_hash.is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;

    fn london() -> Header {
        Header {
            base_fee_per_gas: Some(25_000_000_000),
            ..Default::default()
        }
    }

    #[test]
    fn the_shape_check_sees_a_post_london_field() {
        let h = london();
        assert!(is_london(&h));

        let mut shanghai = h.clone();
        shanghai.withdrawals_root = Some(B256::ZERO);
        assert!(!is_london(&shanghai));

        let mut cancun = h.clone();
        cancun.blob_gas_used = Some(0);
        assert!(!is_london(&cancun));
    }

    #[test]
    fn a_post_london_field_changes_the_name_of_the_block() {
        // The field set is not decoration: adding one renames every block. This
        // is why the shape check is worth having.
        let h = london();
        let mut shanghai = h.clone();
        shanghai.withdrawals_root = Some(B256::ZERO);
        assert_ne!(seal(&h), seal(&shanghai));
    }

    #[test]
    fn the_seal_is_the_hash_of_the_encoding_and_nothing_else() {
        let h = london();
        assert_eq!(seal(&h), keccak256(alloy_rlp::encode(&h)));
    }

    #[test]
    fn a_plain_shape_encodes_as_ethereum_does() {
        // For the shapes Ethereum shares, the layout is alloy's own encoding.
        let mut cancun = london();
        cancun.withdrawals_root = Some(B256::repeat_byte(2));
        cancun.blob_gas_used = Some(0);
        cancun.excess_blob_gas = Some(0);
        cancun.parent_beacon_block_root = Some(B256::repeat_byte(3));
        for h in [Header::default(), london(), cancun] {
            let lux = Lux::genesis(h.clone());
            assert_eq!(lux.encoded(), alloy_rlp::encode(&h), "{}", lux.layout.name);
        }
    }

    #[test]
    fn a_built_block_reads_back_as_itself() {
        for cost in [U256::ZERO, U256::from(200_000u64)] {
            let built = Lux::block(london(), cost);
            let back = Lux::read(&built.encoded()).expect("reads");
            assert_eq!(back, built, "{}", built.layout.name);
        }
        assert_eq!(Lux::block(london(), U256::ZERO).fields(), 17);
        assert_eq!(Lux::block(london(), U256::from(1u64)).fields(), 19);
    }

    #[test]
    fn a_post_shanghai_block_is_in_the_shape_go_encodes() {
        let mut cancun = london();
        cancun.withdrawals_root = Some(crate::root::EMPTY);
        cancun.blob_gas_used = Some(0);
        cancun.excess_blob_gas = Some(0);
        cancun.parent_beacon_block_root = Some(B256::ZERO);
        let plain = Lux::block(cancun.clone(), U256::ZERO);
        assert_eq!(plain.layout.name, "20 eth");
        assert_eq!(
            plain.encoded(),
            alloy_rlp::encode(&cancun),
            "Ethereum's own encoding"
        );
        let owing = Lux::block(cancun.clone(), U256::from(200_000u64));
        assert_eq!(owing.layout.name, "23");
        assert_eq!(Lux::read(&owing.encoded()).expect("reads"), owing);

        let mut shanghai = london();
        shanghai.withdrawals_root = Some(crate::root::EMPTY);
        assert_eq!(
            Lux::block(shanghai.clone(), U256::ZERO).layout.name,
            "17, withdrawals"
        );
        let owing = Lux::block(shanghai, U256::from(1u64));
        assert_eq!(owing.layout.name, "23");
        assert_eq!(owing.fields(), 23);
    }

    #[test]
    #[should_panic(expected = "has no position for blockGasCost")]
    fn a_field_the_shape_has_no_position_for_is_not_dropped() {
        let mut h = Lux::block(london(), U256::ZERO);
        h.block_gas_cost = Some(U256::from(1u64));
        let _ = h.encoded();
    }
}
