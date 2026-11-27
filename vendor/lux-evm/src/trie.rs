// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// See the file LICENSE for licensing terms.

//! A Merkle-Patricia trie whose nodes never change once made.
//!
//! A change makes new nodes along the changed key's path and shares every
//! other node with the trie it came from. So a copy is one pointer, and a
//! root costs the nodes made since the last one: each node hashes itself once,
//! the first time it is asked, and keeps the answer — which can never go stale,
//! because the node it describes can never change.
//!
//! The encoding is Ethereum's (the Yellow Paper, appendix D): a leaf is
//! `[hp(path, leaf), value]`, an extension `[hp(path), child]`, a branch its
//! sixteen children and an empty value; a child whose encoding is shorter than
//! 32 bytes is carried inline, any other by its keccak256. Keys are 32 bytes —
//! the hashes the state and storage tries are keyed by — so no key is a prefix
//! of another and no branch ever holds a value. `alloy-trie` builds the same
//! root from sorted leaves, and the tests hold the two to each other.

use std::sync::{Arc, OnceLock};

use alloy_primitives::{keccak256, B256};
use alloy_rlp::EMPTY_STRING_CODE;

/// What a trie holds at a key: something that knows the bytes it commits to.
pub trait Value: Clone {
    /// The RLP a leaf carries for this value.
    fn rlp(&self) -> Vec<u8>;
}

/// A key's 64 nibbles.
type Path = Box<[u8]>;

/// How a parent names a child: the child's RLP when it is shorter than 32
/// bytes, its keccak256 otherwise.
#[derive(Clone, Copy)]
struct Ref {
    len: u8,
    bytes: [u8; 32],
}

impl Ref {
    fn of(rlp: &[u8]) -> Self {
        let mut bytes = [0u8; 32];
        if rlp.len() < 32 {
            bytes[..rlp.len()].copy_from_slice(rlp);
            return Self {
                len: rlp.len() as u8,
                bytes,
            };
        }
        Self {
            len: 32,
            bytes: keccak256(rlp).0,
        }
    }

    /// As an item of the parent's list.
    fn put(&self, out: &mut Vec<u8>) {
        if self.len == 32 {
            out.push(EMPTY_STRING_CODE + 32);
        }
        out.extend_from_slice(&self.bytes[..self.len as usize]);
    }
}

enum Node<V> {
    Leaf {
        path: Path,
        value: V,
        name: OnceLock<Ref>,
    },
    Extension {
        path: Path,
        child: Arc<Node<V>>,
        name: OnceLock<Ref>,
    },
    Branch {
        children: [Option<Arc<Node<V>>>; 16],
        name: OnceLock<Ref>,
    },
}

impl<V: Value> Node<V> {
    fn leaf(path: &[u8], value: V) -> Arc<Self> {
        Arc::new(Node::Leaf {
            path: path.into(),
            value,
            name: OnceLock::new(),
        })
    }

    /// `path` above `child`: the child itself when the path is empty, and one
    /// node rather than two when the child has a path of its own.
    fn under(path: &[u8], child: Arc<Self>) -> Arc<Self> {
        if path.is_empty() {
            return child;
        }
        match &*child {
            Node::Leaf {
                path: rest, value, ..
            } => Node::leaf(&[path, &**rest].concat(), value.clone()),
            Node::Extension {
                path: rest, child, ..
            } => Arc::new(Node::Extension {
                path: [path, &**rest].concat().into(),
                child: child.clone(),
                name: OnceLock::new(),
            }),
            Node::Branch { .. } => Arc::new(Node::Extension {
                path: path.into(),
                child,
                name: OnceLock::new(),
            }),
        }
    }

    fn branch(children: [Option<Arc<Self>>; 16]) -> Arc<Self> {
        Arc::new(Node::Branch {
            children,
            name: OnceLock::new(),
        })
    }

    fn get(&self, path: &[u8]) -> Option<&V> {
        match self {
            Node::Leaf { path: p, value, .. } => (**p == *path).then_some(value),
            Node::Extension { path: p, child, .. } => {
                path.strip_prefix(&**p).and_then(|rest| child.get(rest))
            }
            Node::Branch { children, .. } => children[path[0] as usize].as_ref()?.get(&path[1..]),
        }
    }

    /// This node with `value` at `path`; whether the key was new.
    fn insert(node: Option<&Arc<Self>>, path: &[u8], value: V) -> (Arc<Self>, bool) {
        let Some(node) = node else {
            return (Node::leaf(path, value), true);
        };
        match &**node {
            Node::Leaf {
                path: p,
                value: old,
                ..
            } => {
                if **p == *path {
                    return (Node::leaf(path, value), false);
                }
                let at = common(p, path);
                let mut children: [Option<Arc<Self>>; 16] = Default::default();
                children[p[at] as usize] = Some(Node::leaf(&p[at + 1..], old.clone()));
                children[path[at] as usize] = Some(Node::leaf(&path[at + 1..], value));
                (Node::under(&path[..at], Node::branch(children)), true)
            }
            Node::Extension { path: p, child, .. } => {
                let at = common(p, path);
                if at == p.len() {
                    let (child, new) = Node::insert(Some(child), &path[at..], value);
                    return (Node::under(p, child), new);
                }
                let mut children: [Option<Arc<Self>>; 16] = Default::default();
                children[p[at] as usize] = Some(Node::under(&p[at + 1..], child.clone()));
                children[path[at] as usize] = Some(Node::leaf(&path[at + 1..], value));
                (Node::under(&path[..at], Node::branch(children)), true)
            }
            Node::Branch { children, .. } => {
                let mut children = children.clone();
                let i = path[0] as usize;
                let (child, new) = Node::insert(children[i].as_ref(), &path[1..], value);
                children[i] = Some(child);
                (Node::branch(children), new)
            }
        }
    }

    /// This node without the key at `path`: `None` when the key is absent and
    /// nothing changed, `Some(None)` when nothing is left.
    fn remove(node: &Arc<Self>, path: &[u8]) -> Option<Option<Arc<Self>>> {
        match &**node {
            Node::Leaf { path: p, .. } => (**p == *path).then_some(None),
            Node::Extension { path: p, child, .. } => {
                let rest = path.strip_prefix(&**p)?;
                Some(Node::remove(child, rest)?.map(|child| Node::under(p, child)))
            }
            Node::Branch { children, .. } => {
                let i = path[0] as usize;
                let child = Node::remove(children[i].as_ref()?, &path[1..])?;
                let mut children = children.clone();
                children[i] = child;
                if children.iter().flatten().count() > 1 {
                    return Some(Some(Node::branch(children)));
                }
                // One child left: a branch of one is a path to that child.
                let (j, only) = children
                    .iter()
                    .enumerate()
                    .find_map(|(j, c)| c.clone().map(|c| (j, c)))
                    .expect("a branch keeps a child");
                Some(Some(Node::under(&[j as u8], only)))
            }
        }
    }

    /// How this node's parent names it, worked out once.
    fn name(&self) -> &Ref {
        let name = match self {
            Node::Leaf { name, .. } | Node::Extension { name, .. } | Node::Branch { name, .. } => {
                name
            }
        };
        name.get_or_init(|| Ref::of(&self.rlp()))
    }

    fn rlp(&self) -> Vec<u8> {
        let mut body = Vec::with_capacity(532);
        match self {
            Node::Leaf { path, value, .. } => {
                string(&prefixed(path, true), &mut body);
                string(&value.rlp(), &mut body);
            }
            Node::Extension { path, child, .. } => {
                string(&prefixed(path, false), &mut body);
                child.name().put(&mut body);
            }
            Node::Branch { children, .. } => {
                for c in children {
                    match c {
                        Some(c) => c.name().put(&mut body),
                        None => body.push(EMPTY_STRING_CODE),
                    }
                }
                body.push(EMPTY_STRING_CODE);
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
}

fn common(a: &[u8], b: &[u8]) -> usize {
    a.iter().zip(b).take_while(|(x, y)| x == y).count()
}

/// Hex-prefix encoding: the flag nibble — 2 for a leaf, 0 for an extension,
/// plus 1 when the path is odd — and then the path packed two to a byte.
fn prefixed(path: &[u8], leaf: bool) -> Vec<u8> {
    let flag = if leaf { 2 } else { 0 };
    let mut out = Vec::with_capacity(path.len() / 2 + 1);
    let rest = if path.len() % 2 == 1 {
        out.push(((flag + 1) << 4) | path[0]);
        &path[1..]
    } else {
        out.push(flag << 4);
        path
    };
    out.extend(rest.chunks(2).map(|p| (p[0] << 4) | p[1]));
    out
}

fn string(bytes: &[u8], out: &mut Vec<u8>) {
    alloy_rlp::Encodable::encode(bytes, out);
}

fn nibbles(key: &B256) -> [u8; 64] {
    let mut out = [0u8; 64];
    for (i, b) in key.iter().enumerate() {
        out[2 * i] = b >> 4;
        out[2 * i + 1] = b & 0x0f;
    }
    out
}

/// A trie from 32-byte keys to values.
pub struct Trie<V> {
    root: Option<Arc<Node<V>>>,
    len: usize,
}

impl<V> Clone for Trie<V> {
    /// One pointer: every node is shared.
    fn clone(&self) -> Self {
        Self {
            root: self.root.clone(),
            len: self.len,
        }
    }
}

impl<V> Default for Trie<V> {
    fn default() -> Self {
        Self { root: None, len: 0 }
    }
}

impl<V: Value> Trie<V> {
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn get(&self, key: &B256) -> Option<&V> {
        self.root.as_ref()?.get(&nibbles(key))
    }

    pub fn insert(&mut self, key: &B256, value: V) {
        let (root, new) = Node::insert(self.root.as_ref(), &nibbles(key), value);
        self.root = Some(root);
        self.len += usize::from(new);
    }

    pub fn remove(&mut self, key: &B256) {
        let Some(root) = &self.root else {
            return;
        };
        if let Some(root) = Node::remove(root, &nibbles(key)) {
            self.root = root;
            self.len -= 1;
        }
    }

    /// The root hash. The empty trie's is keccak256 of the empty string.
    pub fn root(&self) -> B256 {
        match &self.root {
            None => alloy_trie::EMPTY_ROOT_HASH,
            Some(node) => {
                let name = node.name();
                if name.len == 32 {
                    B256::from(name.bytes)
                } else {
                    keccak256(&name.bytes[..name.len as usize])
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::U256;
    use alloy_trie::{HashBuilder, Nibbles};
    use std::collections::BTreeMap;

    #[derive(Clone, Debug, PartialEq)]
    struct Word(U256);

    impl Value for Word {
        fn rlp(&self) -> Vec<u8> {
            alloy_rlp::encode(self.0)
        }
    }

    /// The root `alloy-trie` builds from the same leaves, sorted.
    fn built(map: &BTreeMap<B256, U256>) -> B256 {
        let mut hb = HashBuilder::default();
        for (k, v) in map {
            hb.add_leaf(Nibbles::unpack(k), &alloy_rlp::encode(v));
        }
        hb.root()
    }

    /// A small deterministic generator, so a failure names its seed.
    fn next(state: &mut u64) -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    }

    /// A key from a narrow family: long shared prefixes make inline children,
    /// extensions and collapses that random 32-byte keys almost never do.
    fn key(state: &mut u64, narrow: bool) -> B256 {
        let mut k = B256::ZERO;
        if narrow {
            k.0[31] = (next(state) % 7) as u8;
            k.0[30] = (next(state) % 3) as u8;
            k.0[2] = (next(state) % 2) as u8 * 0x11;
        } else {
            for b in k.0.iter_mut() {
                *b = next(state) as u8;
            }
        }
        k
    }

    #[test]
    fn every_root_along_random_inserts_and_removes_is_the_one_alloy_builds() {
        for seed in 1..=40u64 {
            let mut s = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15);
            let narrow = seed.is_multiple_of(2);
            let mut trie: Trie<Word> = Trie::default();
            let mut map = BTreeMap::new();
            let mut kept = Vec::new();
            for step in 0..300 {
                let k = key(&mut s, narrow);
                if next(&mut s).is_multiple_of(3) {
                    trie.remove(&k);
                    map.remove(&k);
                } else {
                    // Small values too, so a leaf can be shorter than a hash.
                    let v = U256::from(next(&mut s) % 1000 + 1);
                    trie.insert(&k, Word(v));
                    map.insert(k, v);
                }
                assert_eq!(trie.len(), map.len(), "seed {seed} step {step}");
                assert_eq!(trie.root(), built(&map), "seed {seed} step {step}");
                if step % 50 == 0 {
                    kept.push((trie.clone(), map.clone()));
                }
            }
            for (k, v) in &map {
                assert_eq!(trie.get(k), Some(&Word(*v)));
            }
            // An earlier copy is the trie it was: nothing since changed it.
            for (t, m) in &kept {
                assert_eq!(t.root(), built(m), "seed {seed}: a copy moved");
            }
            // Emptied, it is the empty trie.
            for k in map.keys() {
                trie.remove(k);
            }
            assert_eq!(trie.root(), alloy_trie::EMPTY_ROOT_HASH);
            assert!(trie.is_empty());
        }
    }

    #[test]
    fn removing_an_absent_key_changes_nothing() {
        let mut t: Trie<Word> = Trie::default();
        t.remove(&B256::ZERO);
        t.insert(&B256::repeat_byte(1), Word(U256::from(5u64)));
        let root = t.root();
        t.remove(&B256::repeat_byte(2));
        assert_eq!((t.root(), t.len()), (root, 1));
    }
}
