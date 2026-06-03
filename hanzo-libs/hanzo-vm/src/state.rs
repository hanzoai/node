//! Account state persistence.
//!
//! [`StateDb`] wraps SQLite for the development phase. The storage layout
//! mirrors an MPT-backed state trie so migration to a production Merkle
//! Patricia Trie is straightforward.
//!
//! Each account is keyed by its hex-encoded address and stores:
//! nonce, balance, code hash, and storage root.

use anyhow::Result;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Account
// ---------------------------------------------------------------------------

/// EVM account state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    /// Transaction count.
    pub nonce: u64,
    /// Balance in wei.
    pub balance: u128,
    /// Keccak-256 hash of the account bytecode (or zero for EOAs).
    pub code_hash: [u8; 32],
    /// Root of the account storage trie.
    pub storage_root: [u8; 32],
}

impl Default for Account {
    fn default() -> Self {
        Self {
            nonce: 0,
            balance: 0,
            code_hash: [0u8; 32],
            storage_root: [0u8; 32],
        }
    }
}

// ---------------------------------------------------------------------------
// StateDb
// ---------------------------------------------------------------------------

/// SQLite-backed account state database.
///
/// This is the development backend. Production deployments will swap in an
/// MPT-backed store behind the same interface.
pub struct StateDb {
    /// Path to the data directory.
    data_dir: PathBuf,
    /// Open SQLite connection (initialized lazily via [`init`]).
    conn: Option<Connection>,
    /// Cached state root (SHA-256 of serialized account set).
    current_root: [u8; 32],
}

impl StateDb {
    /// Create a new `StateDb` targeting the given directory.
    ///
    /// The database file is created lazily on [`init`].
    pub fn new(data_dir: &str) -> Self {
        Self {
            data_dir: PathBuf::from(data_dir),
            conn: None,
            current_root: [0u8; 32],
        }
    }

    /// Open (or create) the database and initialize the schema.
    pub fn init(&mut self) -> Result<()> {
        std::fs::create_dir_all(&self.data_dir)?;
        let db_path = self.data_dir.join("state.db");
        let conn = Connection::open(db_path)?;

        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS accounts (
                address  TEXT PRIMARY KEY,
                nonce    INTEGER NOT NULL DEFAULT 0,
                balance  TEXT    NOT NULL DEFAULT '0',
                code_hash BLOB  NOT NULL,
                storage_root BLOB NOT NULL
            );

            CREATE TABLE IF NOT EXISTS storage (
                address TEXT NOT NULL,
                slot    TEXT NOT NULL,
                value   BLOB NOT NULL,
                PRIMARY KEY (address, slot)
            );
            ",
        )?;

        self.conn = Some(conn);
        Ok(())
    }

    /// Lightweight connectivity check for health probes.
    pub fn ping(&self) -> Result<()> {
        let conn = self
            .conn
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("state DB not initialized"))?;
        conn.execute_batch("SELECT 1")?;
        Ok(())
    }

    /// Return the current state root hash.
    pub fn root(&self) -> [u8; 32] {
        self.current_root
    }

    /// Retrieve an account by hex address.
    ///
    /// Returns `Account::default()` if the address has no state.
    pub fn get_account(&self, address: &str) -> Result<Account> {
        let conn = self
            .conn
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("state DB not initialized"))?;

        let mut stmt = conn.prepare(
            "SELECT nonce, balance, code_hash, storage_root FROM accounts WHERE address = ?1",
        )?;

        let result = stmt.query_row([address], |row| {
            let nonce: u64 = row.get(0)?;
            let balance_str: String = row.get(1)?;
            let code_hash: Vec<u8> = row.get(2)?;
            let storage_root: Vec<u8> = row.get(3)?;
            Ok((nonce, balance_str, code_hash, storage_root))
        });

        match result {
            Ok((nonce, balance_str, code_hash, storage_root)) => {
                let balance: u128 = balance_str
                    .parse()
                    .map_err(|_| anyhow::anyhow!("corrupt balance for {address}"))?;
                let mut ch = [0u8; 32];
                let mut sr = [0u8; 32];
                if code_hash.len() == 32 {
                    ch.copy_from_slice(&code_hash);
                }
                if storage_root.len() == 32 {
                    sr.copy_from_slice(&storage_root);
                }
                Ok(Account {
                    nonce,
                    balance,
                    code_hash: ch,
                    storage_root: sr,
                })
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(Account::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// Insert or update an account.
    pub fn set_account(&mut self, address: &str, account: &Account) -> Result<()> {
        let conn = self
            .conn
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("state DB not initialized"))?;

        conn.execute(
            "INSERT INTO accounts (address, nonce, balance, code_hash, storage_root)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(address) DO UPDATE SET
                nonce = excluded.nonce,
                balance = excluded.balance,
                code_hash = excluded.code_hash,
                storage_root = excluded.storage_root",
            rusqlite::params![
                address,
                account.nonce,
                account.balance.to_string(),
                account.code_hash.as_slice(),
                account.storage_root.as_slice(),
            ],
        )?;

        self.recompute_root()?;
        Ok(())
    }

    /// Read a storage slot for the given account.
    pub fn get_storage(&self, address: &str, slot: &str) -> Result<Vec<u8>> {
        let conn = self
            .conn
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("state DB not initialized"))?;

        let mut stmt =
            conn.prepare("SELECT value FROM storage WHERE address = ?1 AND slot = ?2")?;

        match stmt.query_row([address, slot], |row| row.get::<_, Vec<u8>>(0)) {
            Ok(v) => Ok(v),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(vec![]),
            Err(e) => Err(e.into()),
        }
    }

    /// Write a storage slot.
    pub fn set_storage(&mut self, address: &str, slot: &str, value: &[u8]) -> Result<()> {
        let conn = self
            .conn
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("state DB not initialized"))?;

        conn.execute(
            "INSERT INTO storage (address, slot, value)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(address, slot) DO UPDATE SET value = excluded.value",
            rusqlite::params![address, slot, value],
        )?;

        self.recompute_root()?;
        Ok(())
    }

    /// Apply a block's state transitions and return the new state root.
    ///
    /// In this development implementation the root is simply a SHA-256
    /// digest of all account rows. A production implementation would
    /// compute a proper Merkle Patricia Trie root.
    pub fn apply_block(&mut self, block: &crate::block::Block) -> Result<[u8; 32]> {
        // For each transaction, a full implementation would execute the EVM
        // and apply resulting state diffs. For now we just bump the root to
        // reflect that a new block was processed.
        let _ = block;
        self.recompute_root()?;
        Ok(self.current_root)
    }

    /// Recompute the state root from the current account set.
    fn recompute_root(&mut self) -> Result<()> {
        let conn = self
            .conn
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("state DB not initialized"))?;

        let mut stmt =
            conn.prepare("SELECT address, nonce, balance, code_hash, storage_root FROM accounts ORDER BY address")?;

        let mut hasher = Sha256::new();
        let rows = stmt.query_map([], |row| {
            let address: String = row.get(0)?;
            let nonce: u64 = row.get(1)?;
            let balance: String = row.get(2)?;
            let code_hash: Vec<u8> = row.get(3)?;
            let storage_root: Vec<u8> = row.get(4)?;
            Ok((address, nonce, balance, code_hash, storage_root))
        })?;

        for row in rows {
            let (address, nonce, balance, code_hash, storage_root) = row?;
            hasher.update(address.as_bytes());
            hasher.update(&nonce.to_le_bytes());
            hasher.update(balance.as_bytes());
            hasher.update(&code_hash);
            hasher.update(&storage_root);
        }

        let digest = hasher.finalize();
        self.current_root.copy_from_slice(&digest);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = StateDb::new(&dir.path().to_string_lossy());
        db.init().unwrap();

        let account = Account {
            nonce: 42,
            balance: 1_000_000_000_000_000_000,
            code_hash: [0xaa; 32],
            storage_root: [0xbb; 32],
        };

        db.set_account("0xdeadbeef", &account).unwrap();
        let loaded = db.get_account("0xdeadbeef").unwrap();
        assert_eq!(account, loaded);
    }

    #[test]
    fn missing_account_returns_default() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = StateDb::new(&dir.path().to_string_lossy());
        db.init().unwrap();

        let loaded = db.get_account("0xnonexistent").unwrap();
        assert_eq!(loaded, Account::default());
    }

    #[test]
    fn storage_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = StateDb::new(&dir.path().to_string_lossy());
        db.init().unwrap();

        db.set_storage("0xaddr", "0x00", &[1, 2, 3]).unwrap();
        let value = db.get_storage("0xaddr", "0x00").unwrap();
        assert_eq!(value, vec![1, 2, 3]);
    }

    #[test]
    fn root_changes_on_write() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = StateDb::new(&dir.path().to_string_lossy());
        db.init().unwrap();

        let root_before = db.root();
        db.set_account("0xaa", &Account::default()).unwrap();
        let root_after = db.root();

        assert_ne!(root_before, root_after);
    }

    #[test]
    fn ping_works_after_init() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = StateDb::new(&dir.path().to_string_lossy());
        db.init().unwrap();
        assert!(db.ping().is_ok());
    }

    #[test]
    fn ping_fails_before_init() {
        let db = StateDb::new("/tmp/no-init");
        assert!(db.ping().is_err());
    }
}
