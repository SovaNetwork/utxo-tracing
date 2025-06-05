use std::sync::Arc;

use chrono::{DateTime, Utc};
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::params;
use rusqlite_migration::{Migrations, M};

use crate::error::{StorageError, StorageResult};
use crate::models::signed_tx::SignedTx;

pub struct SignedTxDatabase {
    conn: Pool<SqliteConnectionManager>,
}

impl SignedTxDatabase {
    pub fn new() -> StorageResult<Arc<Self>> {
        let data_dir = std::env::current_dir()
            .map_err(|e| StorageError::IoError(e.to_string()))?
            .join("data");
        std::fs::create_dir_all(&data_dir).map_err(|e| StorageError::IoError(e.to_string()))?;
        let db_path = data_dir.join("signed.db");
        let db_str = db_path
            .to_str()
            .ok_or_else(|| StorageError::UnexpectedError("Invalid database path".to_string()))?;
        let manager = SqliteConnectionManager::file(db_str);
        let conn = Pool::new(manager)
            .map_err(|e| StorageError::DatabaseConnectionFailed(e.to_string()))?;
        let db = Arc::new(Self { conn });
        db.run_migrations()?;
        Ok(db)
    }

    fn run_migrations(&self) -> StorageResult<()> {
        let migrations = Migrations::new(vec![
            M::up(
                "CREATE TABLE IF NOT EXISTS signed_transactions (\n                    id INTEGER PRIMARY KEY AUTOINCREMENT,\n                    txid TEXT NOT NULL,\n                    signed_tx TEXT NOT NULL,\n                    caller TEXT NOT NULL,\n                    block_height INTEGER NOT NULL,\n                    amount INTEGER NOT NULL,\n                    destination TEXT NOT NULL,\n                    fee INTEGER NOT NULL,\n                    timestamp TEXT NOT NULL\n                )",
            ),
            M::up("CREATE INDEX IF NOT EXISTS idx_signed_caller ON signed_transactions(caller)"),
            M::up("CREATE INDEX IF NOT EXISTS idx_signed_block ON signed_transactions(block_height)"),
        ]);

        let mut conn = self
            .conn
            .get()
            .map_err(|e| StorageError::DatabaseConnectionFailed(e.to_string()))?;
        migrations
            .to_latest(&mut conn)
            .map_err(|e| StorageError::MigrationFailed(e.to_string()))?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn store_signed_tx(
        &self,
        txid: &str,
        signed_tx: &str,
        caller: &str,
        block_height: i32,
        amount: i64,
        destination: &str,
        fee: i64,
    ) -> StorageResult<()> {
        let conn = self
            .conn
            .get()
            .map_err(|e| StorageError::DatabaseConnectionFailed(e.to_string()))?;
        conn.execute(
            "INSERT INTO signed_transactions (txid, signed_tx, caller, block_height, amount, destination, fee, timestamp) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                txid,
                signed_tx,
                caller,
                block_height,
                amount,
                destination,
                fee,
                Utc::now().to_rfc3339()
            ],
        )
        .map_err(|e| StorageError::DatabaseQueryFailed(e.to_string()))?;
        Ok(())
    }

    pub fn get_signed_tx_by_txid(&self, txid: &str) -> StorageResult<Option<SignedTx>> {
        let conn = self
            .conn
            .get()
            .map_err(|e| StorageError::DatabaseConnectionFailed(e.to_string()))?;
        let mut stmt = conn
            .prepare(
                "SELECT id, txid, signed_tx, caller, block_height, amount, destination, fee, timestamp FROM signed_transactions WHERE txid = ?1 ORDER BY id DESC LIMIT 1",
            )
            .map_err(|e| StorageError::DatabaseQueryFailed(e.to_string()))?;

        let result = stmt
            .query_row(params![txid], |row| {
                let ts: String = row.get(8)?;
                let ts = DateTime::parse_from_rfc3339(&ts)
                    .map_err(|e| rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e)))?
                    .with_timezone(&Utc);
                Ok(SignedTx {
                    id: row.get(0)?,
                    txid: row.get(1)?,
                    signed_tx: row.get(2)?,
                    caller: row.get(3)?,
                    block_height: row.get(4)?,
                    amount: row.get(5)?,
                    destination: row.get(6)?,
                    fee: row.get(7)?,
                    timestamp: ts,
                })
            })
            .optional()
            .map_err(|e| StorageError::DatabaseQueryFailed(e.to_string()))?;
        Ok(result)
    }

    pub fn get_signed_txs_by_caller(&self, caller: &str) -> StorageResult<Vec<SignedTx>> {
        let conn = self
            .conn
            .get()
            .map_err(|e| StorageError::DatabaseConnectionFailed(e.to_string()))?;
        let mut stmt = conn
            .prepare(
                "SELECT id, txid, signed_tx, caller, block_height, amount, destination, fee, timestamp FROM signed_transactions WHERE caller = ?1 ORDER BY id",
            )
            .map_err(|e| StorageError::DatabaseQueryFailed(e.to_string()))?;

        let rows = stmt
            .query_map(params![caller], |row| {
                let ts: String = row.get(8)?;
                let ts = DateTime::parse_from_rfc3339(&ts)
                    .map_err(|e| rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e)))?
                    .with_timezone(&Utc);
                Ok(SignedTx {
                    id: row.get(0)?,
                    txid: row.get(1)?,
                    signed_tx: row.get(2)?,
                    caller: row.get(3)?,
                    block_height: row.get(4)?,
                    amount: row.get(5)?,
                    destination: row.get(6)?,
                    fee: row.get(7)?,
                    timestamp: ts,
                })
            })
            .map_err(|e| StorageError::DatabaseQueryFailed(e.to_string()))?;

        let mut results = Vec::new();
        for r in rows {
            results.push(r.map_err(|e| StorageError::DatabaseQueryFailed(e.to_string()))?);
        }
        Ok(results)
    }

    pub fn get_signed_txs_by_block_height(&self, height: i32) -> StorageResult<Vec<SignedTx>> {
        let conn = self
            .conn
            .get()
            .map_err(|e| StorageError::DatabaseConnectionFailed(e.to_string()))?;
        let mut stmt = conn
            .prepare(
                "SELECT id, txid, signed_tx, caller, block_height, amount, destination, fee, timestamp FROM signed_transactions WHERE block_height = ?1 ORDER BY id",
            )
            .map_err(|e| StorageError::DatabaseQueryFailed(e.to_string()))?;

        let rows = stmt
            .query_map(params![height], |row| {
                let ts: String = row.get(8)?;
                let ts = DateTime::parse_from_rfc3339(&ts)
                    .map_err(|e| rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e)))?
                    .with_timezone(&Utc);
                Ok(SignedTx {
                    id: row.get(0)?,
                    txid: row.get(1)?,
                    signed_tx: row.get(2)?,
                    caller: row.get(3)?,
                    block_height: row.get(4)?,
                    amount: row.get(5)?,
                    destination: row.get(6)?,
                    fee: row.get(7)?,
                    timestamp: ts,
                })
            })
            .map_err(|e| StorageError::DatabaseQueryFailed(e.to_string()))?;

        let mut results = Vec::new();
        for r in rows {
            results.push(r.map_err(|e| StorageError::DatabaseQueryFailed(e.to_string()))?);
        }
        Ok(results)
    }

    pub fn get_signed_txs_by_destination(&self, dest: &str) -> StorageResult<Vec<SignedTx>> {
        let conn = self
            .conn
            .get()
            .map_err(|e| StorageError::DatabaseConnectionFailed(e.to_string()))?;
        let mut stmt = conn
            .prepare(
                "SELECT id, txid, signed_tx, caller, block_height, amount, destination, fee, timestamp FROM signed_transactions WHERE destination = ?1 ORDER BY id",
            )
            .map_err(|e| StorageError::DatabaseQueryFailed(e.to_string()))?;

        let rows = stmt
            .query_map(params![dest], |row| {
                let ts: String = row.get(8)?;
                let ts = DateTime::parse_from_rfc3339(&ts)
                    .map_err(|e| rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e)))?
                    .with_timezone(&Utc);
                Ok(SignedTx {
                    id: row.get(0)?,
                    txid: row.get(1)?,
                    signed_tx: row.get(2)?,
                    caller: row.get(3)?,
                    block_height: row.get(4)?,
                    amount: row.get(5)?,
                    destination: row.get(6)?,
                    fee: row.get(7)?,
                    timestamp: ts,
                })
            })
            .map_err(|e| StorageError::DatabaseQueryFailed(e.to_string()))?;

        let mut results = Vec::new();
        for r in rows {
            results.push(r.map_err(|e| StorageError::DatabaseQueryFailed(e.to_string()))?);
        }
        Ok(results)
    }

    pub fn get_all_signed_txs(&self) -> StorageResult<Vec<SignedTx>> {
        let conn = self
            .conn
            .get()
            .map_err(|e| StorageError::DatabaseConnectionFailed(e.to_string()))?;
        let mut stmt = conn
            .prepare(
                "SELECT id, txid, signed_tx, caller, block_height, amount, destination, fee, timestamp FROM signed_transactions ORDER BY id",
            )
            .map_err(|e| StorageError::DatabaseQueryFailed(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| {
                let ts: String = row.get(8)?;
                let ts = DateTime::parse_from_rfc3339(&ts)
                    .map_err(|e| rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e)))?
                    .with_timezone(&Utc);
                Ok(SignedTx {
                    id: row.get(0)?,
                    txid: row.get(1)?,
                    signed_tx: row.get(2)?,
                    caller: row.get(3)?,
                    block_height: row.get(4)?,
                    amount: row.get(5)?,
                    destination: row.get(6)?,
                    fee: row.get(7)?,
                    timestamp: ts,
                })
            })
            .map_err(|e| StorageError::DatabaseQueryFailed(e.to_string()))?;
        let mut results = Vec::new();
        for r in rows {
            results.push(r.map_err(|e| StorageError::DatabaseQueryFailed(e.to_string()))?);
        }
        Ok(results)
    }

    pub fn get_signed_txs_paginated(
        &self,
        offset: i64,
        limit: i64,
    ) -> StorageResult<Vec<SignedTx>> {
        let conn = self
            .conn
            .get()
            .map_err(|e| StorageError::DatabaseConnectionFailed(e.to_string()))?;
        let mut stmt = conn
            .prepare(
                "SELECT id, txid, signed_tx, caller, block_height, amount, destination, fee, timestamp FROM signed_transactions ORDER BY id LIMIT ?1 OFFSET ?2",
            )
            .map_err(|e| StorageError::DatabaseQueryFailed(e.to_string()))?;
        let rows = stmt
            .query_map(params![limit, offset], |row| {
                let ts: String = row.get(8)?;
                let ts = DateTime::parse_from_rfc3339(&ts)
                    .map_err(|e| rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e)))?
                    .with_timezone(&Utc);
                Ok(SignedTx {
                    id: row.get(0)?,
                    txid: row.get(1)?,
                    signed_tx: row.get(2)?,
                    caller: row.get(3)?,
                    block_height: row.get(4)?,
                    amount: row.get(5)?,
                    destination: row.get(6)?,
                    fee: row.get(7)?,
                    timestamp: ts,
                })
            })
            .map_err(|e| StorageError::DatabaseQueryFailed(e.to_string()))?;

        let mut results = Vec::new();
        for r in rows {
            results.push(r.map_err(|e| StorageError::DatabaseQueryFailed(e.to_string()))?);
        }
        Ok(results)
    }
}
