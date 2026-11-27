//! The signing record, across a restart.
//!
//! One statement per slot is a rule a map enforces inside one process. These
//! check the part a map cannot: that the rule survives the process ending, that
//! a torn write does not become a statement, and that a validator which crashed
//! between recording and sending can still send.

use lux_consensus::finality::{canonical_vote_message, Position, VOTE_MESSAGE_LEN};
use lux_consensus::vote::Slot;
use lux_node::{Journal, Refused};

/// A directory that removes itself.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new(tag: &str) -> Scratch {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("lux-journal-{}-{tag}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch");
        Scratch(dir)
    }

    fn file(&self) -> std::path::PathBuf {
        self.0.join("votes")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn position(height: u64, round: u32) -> Position {
    Position { chain_id: [0xc1; 32], height, round, block_id: [0xb1; 32], ..Default::default() }
}

#[test]
fn a_statement_is_admitted_once_and_stands() {
    let scratch = Scratch::new("stands");
    let mut journal = Journal::open(scratch.file()).expect("open");
    assert!(journal.is_empty());

    let message = canonical_vote_message(&position(9, 1), true);
    journal.admit(&message).expect("a first statement");
    assert_eq!(journal.len(), 1);
    assert_eq!(journal.standing(&Slot::read(&message).unwrap()), Some(&message[..]));

    // Saying the same thing again is not saying two things.
    journal.admit(&message).expect("the identical statement");
    assert_eq!(journal.len(), 1);
}

#[test]
fn a_contradiction_at_one_slot_is_refused() {
    let scratch = Scratch::new("contradiction");
    let mut journal = Journal::open(scratch.file()).expect("open");

    let yes = canonical_vote_message(&position(9, 1), true);
    let no = canonical_vote_message(&position(9, 1), false);
    journal.admit(&yes).expect("a first statement");

    match journal.admit(&no) {
        Err(Refused::Equivocation(standing)) => {
            assert_eq!(standing.as_ref(), &yes[..], "the refusal carries what to re-send")
        }
        other => panic!("a contradiction must be refused, got {other:?}"),
    }
    assert_eq!(journal.len(), 1, "the refused statement was not recorded");
}

#[test]
fn a_different_block_at_one_height_and_round_is_a_contradiction() {
    let scratch = Scratch::new("fork");
    let mut journal = Journal::open(scratch.file()).expect("open");

    let a = canonical_vote_message(&Position { canonical_id: [0xaa; 32], ..position(9, 1) }, true);
    let b = canonical_vote_message(&Position { canonical_id: [0xbb; 32], ..position(9, 1) }, true);
    journal.admit(&a).expect("a first statement");
    assert!(matches!(journal.admit(&b), Err(Refused::Equivocation(_))), "two blocks, one slot");
}

#[test]
fn a_different_round_is_a_different_slot() {
    let scratch = Scratch::new("rounds");
    let mut journal = Journal::open(scratch.file()).expect("open");
    journal.admit(&canonical_vote_message(&position(9, 1), true)).expect("round 1");
    journal.admit(&canonical_vote_message(&position(9, 2), true)).expect("round 2");
    journal.admit(&canonical_vote_message(&position(10, 1), true)).expect("height 10");
    assert_eq!(journal.len(), 3);
}

#[test]
fn the_record_survives_the_process_ending() {
    let scratch = Scratch::new("restart");
    let yes = canonical_vote_message(&position(9, 1), true);
    let no = canonical_vote_message(&position(9, 1), false);

    {
        let mut journal = Journal::open(scratch.file()).expect("open");
        journal.admit(&yes).expect("a first statement");
    } // the process ends here

    let mut restarted = Journal::open(scratch.file()).expect("reopen");
    assert_eq!(restarted.len(), 1, "a restart remembers");
    assert!(
        matches!(restarted.admit(&no), Err(Refused::Equivocation(_))),
        "a node that forgets comes back willing to contradict itself"
    );
    restarted.admit(&yes).expect("and can still re-send what it said");
}

#[test]
fn a_torn_write_is_not_a_statement() {
    let scratch = Scratch::new("torn");
    let yes = canonical_vote_message(&position(9, 1), true);
    {
        let mut journal = Journal::open(scratch.file()).expect("open");
        journal.admit(&yes).expect("a whole statement");
    }

    // A crash mid-append: half an entry on the end. Under write-first ordering
    // that half was never signed, so dropping it is safe — and it must be
    // dropped, or every later append lands off the boundary.
    let mut raw = std::fs::read(scratch.file()).expect("read");
    let whole = raw.len();
    raw.extend_from_slice(&canonical_vote_message(&position(9, 2), true)[..40]);
    std::fs::write(scratch.file(), &raw).expect("write");

    let mut journal = Journal::open(scratch.file()).expect("reopen");
    assert_eq!(journal.len(), 1, "the torn tail is not a statement");
    assert_eq!(
        std::fs::metadata(scratch.file()).expect("stat").len(),
        whole as u64,
        "the file is cut back to the boundary"
    );

    // And the slot the torn entry named is still free.
    journal.admit(&canonical_vote_message(&position(9, 2), true)).expect("round 2 is free");
    assert_eq!(journal.len(), 2);
}

#[test]
fn a_file_holding_two_statements_at_one_slot_is_refused() {
    let scratch = Scratch::new("corrupt");
    let yes = canonical_vote_message(&position(9, 1), true);
    let no = canonical_vote_message(&position(9, 1), false);
    let mut raw = yes.clone();
    raw.extend_from_slice(&no);
    std::fs::write(scratch.file(), &raw).expect("write");

    // Guessing which one this validator actually sent is the one thing a
    // signing record must never do.
    assert!(Journal::open(scratch.file()).is_err(), "corruption is not a crash");
}

#[test]
fn bytes_this_network_disowns_are_not_admitted_and_not_recorded() {
    let scratch = Scratch::new("disowned");
    let mut journal = Journal::open(scratch.file()).expect("open");

    assert!(matches!(journal.admit(&[]), Err(Refused::NotAStatement)));
    assert!(matches!(journal.admit(&[0u8; VOTE_MESSAGE_LEN]), Err(Refused::NotAStatement)));

    let mut wrong_tag = canonical_vote_message(&position(9, 1), true);
    wrong_tag[0] ^= 0xFF;
    assert!(matches!(journal.admit(&wrong_tag), Err(Refused::NotAStatement)));

    assert!(journal.is_empty());
    assert_eq!(std::fs::metadata(scratch.file()).expect("stat").len(), 0, "nothing was written");
}
