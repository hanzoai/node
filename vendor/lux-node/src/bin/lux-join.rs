// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// See the file LICENSE for licensing terms.

//! Join a running luxd network as a peer, and say exactly how far it got.
//!
//! Every layer reports separately, because "it did not work" is four different
//! problems: TLS, the handshake luxd validates, the PeerList that makes the
//! connection count, and whatever is exchanged after.

use std::net::{IpAddr, SocketAddr};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use blst::min_pk::SecretKey;
use lux_evm::header::ParentOf as _;
use lux_node::luxd::{Client, Join, Peer, Staking};

/// Recover the position luxd signed, then say what it was.
///
/// The signature is the oracle. Every field of a `Position` is either known
/// from the block or drawn from a small set, so the honest way to learn the
/// rest is to build candidates and ask luxd's own signature which one it was
/// signing. A candidate that verifies is not a guess that worked — it is the
/// only message that signature could have been made over.
fn solve(dir: &str, chain_hex: &str, pubkey_hex: &str) -> ExitCode {
    use lux_consensus::finality::{canonical_vote_message, Position};

    let payload =
        std::fs::read(format!("{dir}/gossip0.kind1.payload.bin")).expect("luxd's vote payload");
    let outer_block =
        std::fs::read(format!("{dir}/gossip0.kind1.block.bin")).expect("the envelope's block id");

    // THE CONTAINER THE VOTE IS ABOUT, not merely the first one captured. A
    // proposervm block's id is the hash of its bytes, so the right container is
    // the one that hashes to the id in the envelope; solving against any other
    // is solving for a different block.
    let mut container = Vec::new();
    for i in 0..8 {
        let Ok(bytes) = std::fs::read(format!("{dir}/container{i}.bin")) else {
            continue;
        };
        let id: [u8; 32] = {
            use sha2::{Digest, Sha256};
            Sha256::digest(&bytes).into()
        };
        if id.as_slice() == outer_block.as_slice() {
            println!("container{i} is the block the vote names");
            container = bytes;
            break;
        }
    }
    if container.is_empty() {
        eprintln!("no captured container hashes to the voted block id");
        return ExitCode::FAILURE;
    }
    let chain: [u8; 32] =
        alloy_primitives::hex::decode(chain_hex).expect("chain id").try_into().expect("32 bytes");
    let pubkey = alloy_primitives::hex::decode(pubkey_hex.trim_start_matches("0x"))
        .expect("luxd's bls public key");
    let (voter, sig) = lux_node::luxd::quorum::read_vote(&payload).expect("a signed vote");
    println!("luxd's vote is by NodeID-{}", lux_node::luxd::cb58(&voter));

    // VALIDATE THE ORACLE FIRST. A search that can never succeed looks exactly
    // like a search over the wrong fields, so the verifier is checked against a
    // signature that is known to be good: luxd's own proof of possession, which
    // is in the genesis and signs its own compressed public key under the PoP
    // tag. If this fails, nothing below means anything.
    let pop_hex = std::env::var("LUXD_POP").unwrap_or_default();
    if !pop_hex.is_empty() {
        let pop = alloy_primitives::hex::decode(pop_hex.trim_start_matches("0x")).expect("pop");
        let ok = {
            let pk = blst::min_pk::PublicKey::uncompress(&pubkey).expect("key");
            let s = blst::min_pk::Signature::uncompress(&pop).expect("sig");
            s.verify(true, &pubkey, lux_node::luxd::POP_DST, &[], &pk, true)
                == blst::BLST_ERROR::BLST_SUCCESS
        };
        println!("oracle self-test (luxd's own PoP verifies): {ok}");
        if !ok {
            eprintln!("the verifier is broken; every search below would fail regardless");
            return ExitCode::FAILURE;
        }
    }

    // The inner execution block. `ids.ID(ethBlock.Hash())` is the C-chain's
    // block id, so the canonical id is keccak of the header's RLP — which is
    // exactly what this crate already computes for a Lux header.
    let Some((inner_start, header_rlp)) = inner_header(&container) else {
        eprintln!("no ethereum header found inside the container");
        return ExitCode::FAILURE;
    };
    let header = lux_evm::header::Lux::read(header_rlp).expect("the inner header parses");
    let canonical = lux_evm::header::seal_bytes(header_rlp);
    println!(
        "inner block at {inner_start}: height {} hash {} parent {}",
        header.header.number, canonical, header.header.parent_hash
    );

    // The outer parent id sits in the container's root, right after the kind
    // byte: ZAP's four magic bytes, three header words, then kind, then
    // parentID.
    let mut outer_parent = [0u8; 32];
    outer_parent.copy_from_slice(&container[17..49]);

    let mut position = Position {
        chain_id: chain,
        height: header.header.number,
        round: 0,
        block_id: outer_block.clone().try_into().expect("32 bytes"),
        parent_id: outer_parent,
        canonical_id: canonical.0,
        parent_canonical_id: header.header.parent_hash.0,
        execution_state_root: [0u8; 32],
        payload_root: [0u8; 32],
        validator_set_root: [0u8; 32],
    };

    // The fields the block does not fix. `hashValidatorSet` is byte-identical
    // to `Committee::root`, so the candidate roots are computed and not
    // guessed; what is unknown is only WHICH set was in force and whether Go
    // hashes the compressed or the uncompressed key.
    let members: Vec<(&str, &str)> = vec![
        ("NodeID-9XacvrcF3HTzCqb8gY4zctu614jQaDDyY", pubkey_hex.trim_start_matches("0x")),
        (
            "NodeID-PEPLweTWhTvZbbMncxPEicgtWV7ydjFTU",
            "84552ed0e1c451dcd8b1c2f1850edb359b931dab48f07b949975e9ce55fa694d73b504f7fac8779ccdd6c9a457f58c24",
        ),
    ];
    // The EFFECTIVE weight, read from luxd's own /validators/at — not the
    // weight the genesis declared. Allocations contribute stake, so the two
    // differ, and the root is over what the P-chain actually computed.
    let weight: u64 =
        std::env::var("LUX_WEIGHT").ok().and_then(|v| v.parse().ok()).unwrap_or(7_500_000_000);
    let mut roots: Vec<([u8; 32], String)> = vec![([0u8; 32], "empty/unbound".into())];
    for uncompressed in [true, false] {
        for take in 1..=members.len() {
            let subset = &members[..take];
            roots.push((
                set_root(subset, weight, uncompressed),
                format!(
                    "{take} validator(s), {} key",
                    if uncompressed { "uncompressed" } else { "compressed" }
                ),
            ));
        }
    }

    // A focused sweep first: every field at its most likely value, and only the
    // round moving. If the round is simply larger than a small search assumed,
    // this finds it in one pass instead of multiplying it by everything else.
    if let Ok(max) = std::env::var("LUX_ROUND_SWEEP") {
        let max: u32 = max.parse().unwrap_or(100_000);
        let mut p = position.clone();
        p.execution_state_root = [0u8; 32];
        p.payload_root = [0u8; 32];
        p.validator_set_root = [0u8; 32];
        for height in [1u64, 0] {
            p.height = height;
            for round in 0..max {
                p.round = round;
                if verify(&pubkey, &canonical_vote_message(&p, true), sig) {
                    println!("SOLVED: height {height} round {round}");
                    return ExitCode::SUCCESS;
                }
            }
        }
        println!("no round in 0..{max} matches at either height");
        return ExitCode::FAILURE;
    }

    // Everything ambiguous, as a product. The signature is the oracle: whatever
    // combination it verifies over is what luxd signed, and the rest are simply
    // not that message.
    let outer_id: [u8; 32] = outer_block.clone().try_into().expect("32 bytes");
    let chains: Vec<([u8; 32], &str)> = vec![(chain, "chain id"), ([0u8; 32], "empty chain id")];
    let canon: Vec<([u8; 32], &str)> =
        vec![(canonical.0, "inner eth hash"), (outer_id, "outer container id")];
    // Zeros included deliberately: `blockPositionLocked` leaves every
    // block-derived field zero when the pending block has no ConsensusBlock
    // yet, and the degrade in `canonicalVoteMessageFor` then binds pos.ParentID
    // — which is itself zero in that case.
    let parents: Vec<([u8; 32], &str)> = vec![
        (header.header.parent_canonical(), "inner parent"),
        (outer_parent, "outer parent"),
        ([0u8; 32], "zero (no consensus block)"),
    ];
    let payloads: Vec<([u8; 32], &str)> = vec![
        ([0u8; 32], "empty payload root"),
        (header.header.transactions_root.0, "transactions root"),
    ];
    // The proposervm reports Empty for the execution root, but a wrapper that
    // reported the real one would look exactly like this search failing, so the
    // state root is a candidate rather than an assumption.
    let execs: Vec<([u8; 32], &str)> =
        vec![([0u8; 32], "empty execution root"), (header.header.state_root.0, "state root")];

    let mut tried = 0usize;
    for (cid, cl) in &chains {
        for (cn, cnl) in &canon {
            for (pa, pal) in &parents {
                for (pl, pll) in &payloads {
                    for (ex, exl) in &execs {
                        for (root, label) in &roots {
                            let round_max: u32 = std::env::var("LUX_ROUND_MAX")
                                .ok()
                                .and_then(|v| v.parse().ok())
                                .unwrap_or(10);
                            for round in 0u32..round_max {
                                for height in 0u64..10 {
                                    position.chain_id = *cid;
                                    position.canonical_id = *cn;
                                    position.parent_canonical_id = *pa;
                                    position.payload_root = *pl;
                                    position.execution_state_root = *ex;
                                    position.validator_set_root = *root;
                                    position.round = round;
                                    position.height = height;
                                    let message = canonical_vote_message(&position, true);
                                    tried += 1;
                                    if verify(&pubkey, &message, sig) {
                                        println!("\nSOLVED after {tried} candidates:");
                                        println!("  chain_id           {cl}");
                                        println!(
                                            "  canonical_id       {cnl} = {}",
                                            alloy_primitives::hex::encode(cn)
                                        );
                                        println!(
                                            "  parent_canonical   {pal} = {}",
                                            alloy_primitives::hex::encode(pa)
                                        );
                                        println!("  payload_root       {pll}");
                                        println!("  execution_root     {exl}");
                                        println!("  validator_set_root {label}");
                                        println!("  round              {round}");
                                        println!("  height             {height}");
                                        println!("\nluxd's signature verifies over this message. It is the position.");
                                        std::fs::write(
                                            format!("{dir}/position.bin"),
                                            canonical_vote_message(&position, true),
                                        )
                                        .expect("record the message");
                                        return ExitCode::SUCCESS;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    println!("\nno candidate matched after {tried} tries — a field differs from luxd's");
    ExitCode::FAILURE
}

/// Find the ethereum header RLP inside a proposervm container.
///
/// The container is ZAP's arena format and the inner block is a byte field in
/// it; rather than parse the arena, the block is found by what it IS — a long
/// RLP list whose first element is a header this crate can read.
fn inner_header(container: &[u8]) -> Option<(usize, &[u8])> {
    for i in 0..container.len().saturating_sub(4) {
        if container[i] != 0xf9 {
            continue;
        }
        let outer_len = u16::from_be_bytes([container[i + 1], container[i + 2]]) as usize;
        if i + 3 + outer_len != container.len() {
            continue;
        }
        // The first element of [header, txs, uncles].
        let h = i + 3;
        if container.get(h) != Some(&0xf9) {
            continue;
        }
        let head_len = u16::from_be_bytes([container[h + 1], container[h + 2]]) as usize;
        let rlp = container.get(h..h + 3 + head_len)?;
        if lux_evm::header::Lux::read(rlp).is_ok() {
            return Some((h, rlp));
        }
    }
    None
}

/// Go's `hashValidatorSet`, over a candidate set.
fn set_root(members: &[(&str, &str)], weight: u64, uncompressed: bool) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut rows: Vec<([u8; 20], Vec<u8>)> = members
        .iter()
        .map(|(name, key)| {
            let raw = bs58::decode(name.trim_start_matches("NodeID-")).into_vec().expect("cb58");
            let mut id = [0u8; 20];
            id.copy_from_slice(&raw[..20]);
            let compressed = alloy_primitives::hex::decode(key).expect("key");
            let bytes = if uncompressed {
                blst::min_pk::PublicKey::uncompress(&compressed).expect("key").serialize().to_vec()
            } else {
                compressed
            };
            (id, bytes)
        })
        .collect();
    rows.sort_by_key(|r| r.0);
    let mut h = Sha256::new();
    for (id, key) in rows {
        h.update(id);
        h.update(weight.to_be_bytes());
        h.update((key.len() as u64).to_be_bytes());
        h.update(&key);
    }
    h.finalize().into()
}

fn verify(pubkey: &[u8], message: &[u8], sig: &[u8]) -> bool {
    let (Ok(pk), Ok(s)) =
        (blst::min_pk::PublicKey::uncompress(pubkey), blst::min_pk::Signature::uncompress(sig))
    else {
        return false;
    };
    s.verify(true, message, lux_consensus::cert::DST, &[], &pk, true)
        == blst::BLST_ERROR::BLST_SUCCESS
}

/// The BLS key this validator votes with, kept beside its staking certificate.
fn bls_key(dir: &str) -> SecretKey {
    let path = std::path::Path::new(dir).join("bls.key");
    match std::fs::read(&path) {
        Ok(raw) => SecretKey::from_bytes(&raw).expect("the bls key on disk"),
        Err(_) => {
            let mut material = [0u8; 32];
            let mut f = std::fs::File::open("/dev/urandom").expect("randomness");
            std::io::Read::read_exact(&mut f, &mut material).expect("randomness");
            let key = SecretKey::key_gen(&material, &[]).expect("key_gen");
            let _ = std::fs::create_dir_all(dir);
            lux_node::pq::write_private(&path, &key.to_bytes()).expect("write the bls key");
            key
        }
    }
}

/// Set by the signal handler, read by the daemon loop.
static RUNNING: AtomicBool = AtomicBool::new(true);

extern "C" fn on_signal(_: libc::c_int) {
    RUNNING.store(false, Ordering::SeqCst);
}

const USAGE: &str = "\
lux-join — a Lux validator on a luxd network

  lux-join <staking-addr> <network-id> <chain-id-hex> <tip-hex>
      Join as a validator and stay. Answers polls, votes on every block it is
      queried about, and reconnects if the link drops — until SIGTERM.

  --identity <dir>     print the genesis `signer` block for this validator
  --pop <keyfile>      the same, for a raw 32-byte BLS secret (luxd's own)
  --solve <dir> <chain> <pubkey>
                       recover the VotePosition luxd signed, from its signature

Environment:
  LUX_STAKER_DIR       where this validator's keys live      (default .lux-staker)
  LUX_RPC              luxd's HTTP address, for the validator set
                                                             (default 127.0.0.1:9650)
  LUX_ADVERTISE_PORT   the port this validator claims        (default 19999)
  LUX_SET_ROOT         override the derived set root         (normally unset)

THE VALIDATOR SET is derived, not configured: this reads it from luxd at
/v1/chain/<P-chain-id>/ops/validators/at?height=0 and hashes it the way Go
does. The weights are the ones the P-chain COMPUTED, which are not the ones a
genesis declared.

A certificate needs THREE distinct signers (presetAlpha=3), so a network of
fewer than four validators cannot form one however correct each vote is.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    match args[0].as_str() {
        "--pop" => return pop(&args),
        "--identity" => return identity(&args),
        "--solve" => return solve(&args[1], &args[2], &args[3]),
        _ => {}
    }
    daemon(&args)
}

/// Print the `signer` block for a raw 32-byte BLS secret — luxd keeps its own
/// at `<data>/staking/signer.key`, and a genesis naming it needs its public
/// half.
fn pop(args: &[String]) -> ExitCode {
    let path = args.get(1).expect("a bls key file");
    let raw = std::fs::read(path).expect("the key file");
    let key = SecretKey::from_bytes(&raw).expect("a 32-byte bls secret");
    let (public, proof) = lux_node::luxd::proof_of_possession(&key);
    print_signer(&public, &proof, None);
    ExitCode::SUCCESS
}

/// Print what a genesis needs in order to name this node a validator.
fn identity(args: &[String]) -> ExitCode {
    let dir = args.get(1).cloned().unwrap_or_else(|| ".lux-staker".into());
    let staking = Staking::open(&dir).expect("a staking identity");
    let (public, proof) = lux_node::luxd::proof_of_possession(&bls_key(&dir));
    print_signer(&public, &proof, Some(&staking.node_name()));
    ExitCode::SUCCESS
}

fn print_signer(public: &[u8], proof: &[u8], node: Option<&str>) {
    println!("{{");
    if let Some(n) = node {
        println!("  \"nodeID\": \"{n}\",");
        println!("  \"rewardAddress\": \"\",");
        println!("  \"delegationFee\": 20000,");
        println!("  \"signer\": {{");
    }
    let pad = if node.is_some() { "  " } else { "" };
    println!("{pad}  \"publicKey\": \"0x{}\",", alloy_primitives::hex::encode(public));
    println!("{pad}  \"proofOfPossession\": \"0x{}\"", alloy_primitives::hex::encode(proof));
    if node.is_some() {
        println!("  }}");
    }
    println!("}}");
}

/// Run as a validator until told to stop.
///
/// Not a one-shot: a validator that exited after a fixed span would have to be
/// looped by hand, and a network cannot be left running with one. This holds
/// the link, answers every poll, votes on every block it is queried about, and
/// redials when the link drops.
fn daemon(args: &[String]) -> ExitCode {
    let at: SocketAddr = args[0].parse().expect("luxd's staking address");
    let network: u32 = args.get(1).map(|s| s.parse().expect("a network id")).unwrap_or(1337);
    let chain: [u8; 32] = args
        .get(2)
        .map(|c| {
            alloy_primitives::hex::decode(c).expect("a chain id").try_into().expect("32 bytes")
        })
        .expect("the chain to validate");
    let tip: [u8; 32] = args
        .get(3)
        .map(|t| alloy_primitives::hex::decode(t).expect("a tip").try_into().expect("32 bytes"))
        .expect("the tip this validator starts from");

    let dir = std::env::var("LUX_STAKER_DIR").unwrap_or_else(|_| ".lux-staker".into());
    let staking = match Staking::open(&dir) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("identity: {e}");
            return ExitCode::FAILURE;
        }
    };
    let bls = bls_key(&dir);
    let rpc: SocketAddr = std::env::var("LUX_RPC")
        .unwrap_or_else(|_| "127.0.0.1:9650".into())
        .parse()
        .expect("luxd's http address");

    unsafe {
        libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
    }

    println!("lux-join validator {}", staking.node_name());
    println!("  luxd mesh     {at}");
    println!("  luxd rpc      {rpc}");
    println!("  chain         {}", alloy_primitives::hex::encode(chain));

    let mut accepted_tip = tip;
    let mut backoff = Duration::from_millis(500);
    while RUNNING.load(Ordering::SeqCst) {
        match serve(at, rpc, network, chain, accepted_tip, &staking, &bls) {
            Ok(latest) => {
                accepted_tip = latest;
                backoff = Duration::from_millis(500);
            }
            Err(why) => {
                if !RUNNING.load(Ordering::SeqCst) {
                    break;
                }
                eprintln!("link: {why}; redialing in {:?}", backoff);
                std::thread::sleep(backoff);
                // Backing off doubles to a ceiling: a luxd that is restarting
                // should not be hammered, and one that is gone should not spin
                // this process.
                backoff = (backoff * 2).min(Duration::from_secs(15));
            }
        }
    }
    println!("stopping");
    ExitCode::SUCCESS
}

/// One connection, held until it breaks or the process is told to stop.
///
/// Returns the tip this validator last answered with, so a redial does not
/// forget what it had already accepted.
fn serve(
    at: SocketAddr,
    rpc: SocketAddr,
    network: u32,
    chain: [u8; 32],
    tip: [u8; 32],
    staking: &Staking,
    bls: &SecretKey,
) -> Result<[u8; 32], String> {
    // THE SET, EVERY TIME. Re-read on each dial rather than cached for the life
    // of the process: a validator set changes, and a root computed once would
    // sign votes for a set that is no longer in force.
    let set_root = match std::env::var("LUX_SET_ROOT") {
        Ok(hex) => alloy_primitives::hex::decode(hex.trim_start_matches("0x"))
            .map_err(|e| format!("LUX_SET_ROOT: {e}"))?
            .try_into()
            .map_err(|_| "LUX_SET_ROOT is 32 bytes".to_string())?,
        Err(_) => {
            let set =
                lux_node::validators::at(rpc, 0).map_err(|e| format!("validator set: {e}"))?;
            let root = lux_node::validators::root(&set);
            println!(
                "  set           {} validators, root {}",
                set.len(),
                alloy_primitives::hex::encode(root)
            );
            root
        }
    };

    let mut peer = Peer::connect(at, staking).map_err(|e| e.to_string())?;
    peer.votes_with(lux_node::luxd::Ballot {
        node: staking.node_id(),
        key: bls.clone(),
        chain,
        set_root,
    });
    peer.track(chain, tip);

    let me = Join {
        network,
        addr: IpAddr::from([127, 0, 0, 1]),
        port: std::env::var("LUX_ADVERTISE_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(19999),
        client: Client::default(),
    };
    peer.send_handshake(staking, bls, &me).map_err(|e| e.to_string())?;

    let mut votes = 0usize;
    let mut polls = 0usize;
    loop {
        if !RUNNING.load(Ordering::SeqCst) {
            return Ok(tip);
        }
        match peer.step() {
            Ok(_) => {}
            // A validator with nothing to answer hears nothing. luxd pings
            // about every 22s, so silence is the normal state between polls —
            // it is not a reason to drop a working link.
            Err(lux_node::luxd::Error::Quiet) => continue,
            Err(e) => return Err(e.to_string()),
        }
        if peer.cast.len() > votes {
            votes = peer.cast.len();
            if let Some((height, block)) = peer.cast.last() {
                println!("  voted height {height} block {}", alloy_primitives::hex::encode(block));
            }
        }
        if peer.queried.len() > polls {
            polls = peer.queried.len();
        }
    }
}
