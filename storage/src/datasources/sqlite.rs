use std::sync::Arc;

use chrono::{DateTime, Utc};
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{params, OptionalExtension};
use rusqlite_migration::{Migrations, M};

use network_shared::UtxoUpdate;

use super::Datasource;

use crate::error::{StorageError, StorageResult};
use crate::models::utxo::PendingChanges;

pub struct UtxoSqliteDatasource {
    conn: Pool<SqliteConnectionManager>,
}

impl UtxoSqliteDatasource {
    pub fn new() -> StorageResult<Arc<Self>> {
        let data_dir = std::env::current_dir()
            .map_err(|e| StorageError::IoError(e.to_string()))?
            .join("data");

        std::fs::create_dir_all(&data_dir).map_err(|e| StorageError::IoError(e.to_string()))?;

        let db_dir = data_dir.join("utxo.db");
        let db_path = db_dir
            .to_str()
            .ok_or_else(|| StorageError::UnexpectedError("Invalid database path".to_string()))?;

        let manager = SqliteConnectionManager::file(db_path);
        let conn = Pool::new(manager)
            .map_err(|e| StorageError::DatabaseConnectionFailed(e.to_string()))?;

        Ok(Arc::new(Self { conn }))
    }

    fn run_migrations(&self) -> StorageResult<()> {
        let migrations = Migrations::new(vec![
            M::up(
                "CREATE TABLE IF NOT EXISTS blocks (
                    height INTEGER NOT NULL PRIMARY KEY,
                    hash TEXT NOT NULL UNIQUE,
                    timestamp TEXT NOT NULL,
                    is_main_chain BOOLEAN NOT NULL DEFAULT 1,
                    is_final BOOLEAN NOT NULL DEFAULT 0
                )",
            ),
            M::up(
                "CREATE TABLE IF NOT EXISTS utxo (
                    vid INTEGER PRIMARY KEY AUTOINCREMENT,
                    id TEXT NOT NULL UNIQUE,
                    address TEXT NOT NULL,
                    public_key TEXT,
                    txid TEXT NOT NULL,
                    vout INTEGER NOT NULL,
                    amount INTEGER NOT NULL,
                    script_pub_key TEXT NOT NULL,
                    script_type TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    block_height INTEGER NOT NULL,
                    spent_txid TEXT,
                    spent_at TEXT,
                    spent_block INTEGER,
                    UNIQUE(txid, vout)
                )",
            ),
            M::up("CREATE INDEX IF NOT EXISTS idx_utxo_address ON utxo(address)"),
            M::up("CREATE INDEX IF NOT EXISTS idx_utxo_block_height ON utxo(block_height)"),
            M::up("CREATE INDEX IF NOT EXISTS idx_utxo_spent_block ON utxo(spent_block)"),
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

    fn upsert_utxo_in_tx(tx: &rusqlite::Transaction, utxo: &UtxoUpdate) -> StorageResult<()> {
        // Validate UTXO
        if utxo.amount < 0 {
            return Err(StorageError::InvalidAmount(utxo.amount));
        }

        if utxo.address.is_empty() {
            return Err(StorageError::InvalidAddress("Empty address".to_string()));
        }

        tx.execute(
            "INSERT INTO utxo (
                id, address, public_key, txid, vout, amount, script_pub_key, 
                script_type, created_at, block_height, spent_txid, spent_at, spent_block
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
            ON CONFLICT(id) DO UPDATE SET
                spent_txid = excluded.spent_txid,
                spent_at = excluded.spent_at,
                spent_block = excluded.spent_block",
            params![
                utxo.id,
                utxo.address,
                utxo.public_key,
                utxo.txid,
                utxo.vout,
                utxo.amount,
                utxo.script_pub_key,
                utxo.script_type,
                utxo.created_at.to_rfc3339(),
                utxo.block_height,
                utxo.spent_txid,
                utxo.spent_at.map(|dt| dt.to_rfc3339()),
                utxo.spent_block,
            ],
        )
        .map_err(|e| StorageError::DatabaseQueryFailed(e.to_string()))?;

        Ok(())
    }
}

impl Datasource for UtxoSqliteDatasource {
    fn setup(&self) -> StorageResult<()> {
        self.run_migrations()
    }

    fn get_type(&self) -> String {
        String::from("Sqlite")
    }

    fn get_latest_block(&self) -> StorageResult<i32> {
        let conn = self
            .conn
            .get()
            .map_err(|e| StorageError::DatabaseConnectionFailed(e.to_string()))?;

        let query = "
            SELECT MAX(height) FROM blocks WHERE is_main_chain = 1;
        ";

        let result = conn
            .query_row(query, [], |row| row.get::<_, Option<i32>>(0))
            .map_err(|e| StorageError::DatabaseQueryFailed(e.to_string()))?;

        // Return 0 if no UTXOs exist yet
        Ok(result.unwrap_or(0))
    }

    fn process_block_utxos(&self, changes: &PendingChanges) -> StorageResult<()> {
        // Validate parameters
        if changes.height < 0 {
            return Err(StorageError::InvalidBlockHeight(changes.height));
        }

        let mut conn = self
            .conn
            .get()
            .map_err(|e| StorageError::DatabaseConnectionFailed(e.to_string()))?;

        // Start a transaction
        let tx = conn.transaction().map_err(|e| {
            StorageError::DatabaseQueryFailed(format!("Failed to start transaction: {}", e))
        })?;

        // Upsert UTXOs from utxos_update
        for utxo in &changes.utxos_update {
            if let Err(e) = UtxoSqliteDatasource::upsert_utxo_in_tx(&tx, utxo) {
                let _ = tx.rollback(); // Try to rollback, but we'll return the original error
                return Err(StorageError::DatabaseQueryFailed(format!(
                    "Failed to upsert UTXO {}: {}",
                    utxo.id, e
                )));
            }
        }

        // Upsert UTXOs from utxos_insert
        for utxo in &changes.utxos_insert {
            if let Err(e) = UtxoSqliteDatasource::upsert_utxo_in_tx(&tx, utxo) {
                let _ = tx.rollback(); // Try to rollback, but we'll return the original error
                return Err(StorageError::DatabaseQueryFailed(format!(
                    "Failed to upsert UTXO {}: {}",
                    utxo.id, e
                )));
            }
        }

        // Commit the transaction
        tx.commit().map_err(|e| {
            StorageError::DatabaseQueryFailed(format!("Failed to commit transaction: {}", e))
        })?;

        Ok(())
    }

    fn get_spendable_utxos_at_height(
        &self,
        block_height: i32,
        address: &str,
    ) -> StorageResult<Vec<UtxoUpdate>> {
        // Validate parameters
        if block_height < 0 {
            return Err(StorageError::InvalidBlockHeight(block_height));
        }

        if address.is_empty() {
            return Err(StorageError::InvalidAddress("Empty address".to_string()));
        }

        let conn = self
            .conn
            .get()
            .map_err(|e| StorageError::DatabaseConnectionFailed(e.to_string()))?;

        let mut stmt = conn
            .prepare(
                "SELECT 
            id, address, public_key, txid, vout, amount, script_pub_key, 
            script_type, created_at, block_height, spent_txid, spent_at, spent_block
            FROM utxo
            WHERE block_height <= ?1 
            AND (spent_block IS NULL OR spent_block > ?1)
            AND address = ?2",
            )
            .map_err(|e| {
                StorageError::DatabaseQueryFailed(format!("Failed to prepare statement: {}", e))
            })?;

        let mut results = Vec::new();

        // Use params! to ensure the types of block_height and address match the placeholders
        let rows = stmt
            .query_map(params![block_height, address], |row| {
                let created_at = row.get::<_, String>(8)?;
                let created_at_parsed = DateTime::parse_from_rfc3339(&created_at)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?
                    .with_timezone(&Utc);

                Ok(UtxoUpdate {
                    id: row.get(0)?,
                    address: row.get(1)?,
                    public_key: row.get(2)?,
                    txid: row.get(3)?,
                    vout: row.get(4)?,
                    amount: row.get(5)?,
                    script_pub_key: row.get(6)?,
                    script_type: row.get(7)?,
                    created_at: created_at_parsed,
                    block_height: row.get(9)?,
                    spent_txid: row.get(10)?,
                    spent_at: row.get::<_, Option<String>>(11)?.map(|s| {
                        DateTime::parse_from_rfc3339(&s)
                            .unwrap()
                            .with_timezone(&Utc)
                    }),
                    spent_block: row.get(12)?,
                })
            })
            .map_err(|e| {
                StorageError::DatabaseQueryFailed(format!("Failed to query rows: {}", e))
            })?;

        for row in rows {
            match row {
                Ok(utxo) => results.push(utxo),
                Err(e) => {
                    return Err(StorageError::DatabaseQueryFailed(format!(
                        "Failed to process row: {}",
                        e
                    )))
                }
            }
        }

        Ok(results)
    }

    fn get_utxos_for_block_and_address(
        &self,
        block_height: i32,
        address: &str,
    ) -> StorageResult<Vec<UtxoUpdate>> {
        // Validate parameters
        if block_height < 0 {
            return Err(StorageError::InvalidBlockHeight(block_height));
        }

        if address.is_empty() {
            return Err(StorageError::InvalidAddress("Empty address".to_string()));
        }

        let conn = self
            .conn
            .get()
            .map_err(|e| StorageError::DatabaseConnectionFailed(e.to_string()))?;

        let query = "
            SELECT 
                id, address, public_key, txid, vout, amount, script_pub_key, 
                script_type, created_at, block_height, spent_txid, spent_at, spent_block
            FROM utxo
            WHERE block_height = ?1 AND address = ?2;
        ";

        // Prepare the statement
        let mut stmt = conn.prepare(query).map_err(|e| {
            StorageError::DatabaseQueryFailed(format!("Failed to prepare statement: {}", e))
        })?;

        // Execute the query and map results to UtxoUpdate
        let rows = stmt
            .query_map(params![block_height, address], |row| {
                let created_at = row.get::<_, String>(8)?;
                let created_at_parsed = DateTime::parse_from_rfc3339(&created_at)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?
                    .with_timezone(&Utc);

                Ok(UtxoUpdate {
                    id: row.get(0)?,
                    address: row.get(1)?,
                    public_key: row.get(2)?,
                    txid: row.get(3)?,
                    vout: row.get(4)?,
                    amount: row.get(5)?,
                    script_pub_key: row.get(6)?,
                    script_type: row.get(7)?,
                    created_at: created_at_parsed,
                    block_height: row.get(9)?,
                    spent_txid: row.get(10)?,
                    spent_at: row.get::<_, Option<String>>(11)?.map(|s| {
                        DateTime::parse_from_rfc3339(&s)
                            .unwrap()
                            .with_timezone(&Utc)
                    }),
                    spent_block: row.get(12)?,
                })
            })
            .map_err(|e| {
                StorageError::DatabaseQueryFailed(format!("Failed to query rows: {}", e))
            })?;

        // Collect the results into a vector
        let mut results = Vec::new();
        for row in rows {
            match row {
                Ok(utxo) => results.push(utxo),
                Err(e) => {
                    return Err(StorageError::DatabaseQueryFailed(format!(
                        "Failed to process row: {}",
                        e
                    )))
                }
            }
        }

        Ok(results)
    }

    fn get_all_utxos_for_block(&self, block_height: i32) -> StorageResult<Vec<UtxoUpdate>> {
        // Validate parameters
        if block_height < 0 {
            return Err(StorageError::InvalidBlockHeight(block_height));
        }

        let conn = self
            .conn
            .get()
            .map_err(|e| StorageError::DatabaseConnectionFailed(e.to_string()))?;

        let query = "
            SELECT 
                id, address, public_key, txid, vout, amount, script_pub_key, 
                script_type, created_at, block_height, spent_txid, spent_at, spent_block
            FROM utxo
            WHERE block_height = ?1;
        ";

        let mut stmt = conn.prepare(query).map_err(|e| {
            StorageError::DatabaseQueryFailed(format!("Failed to prepare statement: {}", e))
        })?;

        let rows = stmt
            .query_map([block_height], |row| {
                let created_at = row.get::<_, String>(8)?;
                let created_at_parsed = DateTime::parse_from_rfc3339(&created_at)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?
                    .with_timezone(&Utc);

                Ok(UtxoUpdate {
                    id: row.get(0)?,
                    address: row.get(1)?,
                    public_key: row.get(2)?,
                    txid: row.get(3)?,
                    vout: row.get(4)?,
                    amount: row.get(5)?,
                    script_pub_key: row.get(6)?,
                    script_type: row.get(7)?,
                    created_at: created_at_parsed,
                    block_height: row.get(9)?,
                    spent_txid: row.get(10)?,
                    spent_at: row.get::<_, Option<String>>(11)?.map(|s| {
                        DateTime::parse_from_rfc3339(&s)
                            .unwrap()
                            .with_timezone(&Utc)
                    }),
                    spent_block: row.get(12)?,
                })
            })
            .map_err(|e| {
                StorageError::DatabaseQueryFailed(format!("Failed to query rows: {}", e))
            })?;

        let mut results = Vec::new();
        for row in rows {
            match row {
                Ok(utxo) => results.push(utxo),
                Err(e) => {
                    return Err(StorageError::DatabaseQueryFailed(format!(
                        "Failed to process row: {}",
                        e
                    )))
                }
            }
        }

        Ok(results)
    }

    fn store_block(&self, height: i32, hash: &str, timestamp: DateTime<Utc>) -> StorageResult<()> {
        let conn = self
            .conn
            .get()
            .map_err(|e| StorageError::DatabaseConnectionFailed(e.to_string()))?;

        conn.execute(
            "INSERT INTO blocks (height, hash, timestamp) 
             VALUES (?1, ?2, ?3)
             ON CONFLICT(height) DO UPDATE SET 
                hash = excluded.hash,
                timestamp = excluded.timestamp,
                is_main_chain = 1",
            params![height, hash, timestamp.to_rfc3339()],
        )
        .map_err(|e| StorageError::DatabaseQueryFailed(e.to_string()))?;

        Ok(())
    }

    fn get_block_hash(&self, height: i32) -> StorageResult<Option<String>> {
        // If height is negative or zero, it's invalid
        if height < 0 {
            return Err(StorageError::InvalidBlockHeight(height));
        }

        // For height 0, if it doesn't exist yet, return an empty string
        // This avoids errors when checking for reorgs before any blocks are processed
        if height == 0 {
            let conn = self
                .conn
                .get()
                .map_err(|e| StorageError::DatabaseConnectionFailed(e.to_string()))?;

            // Check if height 0 exists in the blocks table
            let count: i32 = conn
                .query_row("SELECT COUNT(*) FROM blocks WHERE height = 0", [], |row| {
                    row.get(0)
                })
                .unwrap_or(0);

            if count == 0 {
                return Ok(Some("".to_string()));
            }
        }

        let conn = self
            .conn
            .get()
            .map_err(|e| StorageError::DatabaseConnectionFailed(e.to_string()))?;

        let result = conn.query_row(
            "SELECT hash FROM blocks WHERE height = ? AND is_main_chain = 1",
            params![height],
            |row| row.get(0),
        );

        match result {
            Ok(hash) => Ok(Some(hash)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(StorageError::DatabaseQueryFailed(e.to_string())),
        }
    }

    fn mark_blocks_as_final(&self, threshold: i32) -> StorageResult<()> {
        let conn = self
            .conn
            .get()
            .map_err(|e| StorageError::DatabaseConnectionFailed(e.to_string()))?;

        conn.execute(
            "UPDATE blocks SET is_final = 1 WHERE height <= ? AND is_final = 0",
            params![threshold],
        )
        .map_err(|e| StorageError::DatabaseQueryFailed(e.to_string()))?;

        Ok(())
    }

    fn mark_blocks_after_height_not_main_chain(&self, height: i32) -> StorageResult<()> {
        let conn = self
            .conn
            .get()
            .map_err(|e| StorageError::DatabaseConnectionFailed(e.to_string()))?;

        conn.execute(
            "UPDATE blocks SET is_main_chain = 0 WHERE height > ?",
            params![height],
        )
        .map_err(|e| StorageError::DatabaseQueryFailed(e.to_string()))?;

        Ok(())
    }

    fn revert_utxos_after_height(&self, height: i32) -> StorageResult<()> {
        let mut conn = self
            .conn
            .get()
            .map_err(|e| StorageError::DatabaseConnectionFailed(e.to_string()))?;

        let tx = conn.transaction().map_err(|e| {
            StorageError::DatabaseQueryFailed(format!("Failed to start transaction: {}", e))
        })?;

        // 1. Unspend UTXOs that were spent after this height
        tx.execute(
            "UPDATE utxo SET 
                spent_txid = NULL, 
                spent_at = NULL, 
                spent_block = NULL 
             WHERE spent_block > ?",
            params![height],
        )
        .map_err(|e| StorageError::DatabaseQueryFailed(e.to_string()))?;

        // 2. Delete UTXOs that were created after this height
        tx.execute("DELETE FROM utxo WHERE block_height > ?", params![height])
            .map_err(|e| StorageError::DatabaseQueryFailed(e.to_string()))?;

        tx.commit().map_err(|e| {
            StorageError::DatabaseQueryFailed(format!("Failed to commit transaction: {}", e))
        })?;

        Ok(())
    }

    fn get_utxo_by_outpoint(&self, txid: &str, vout: i32) -> StorageResult<Option<UtxoUpdate>> {
        let conn = self
            .conn
            .get()
            .map_err(|e| StorageError::DatabaseConnectionFailed(e.to_string()))?;

        let mut stmt = conn
            .prepare(
                "SELECT id, address, public_key, txid, vout, amount, script_pub_key, script_type, created_at, block_height, spent_txid, spent_at, spent_block FROM utxo WHERE txid = ?1 AND vout = ?2 LIMIT 1",
            )
            .map_err(|e| StorageError::DatabaseQueryFailed(e.to_string()))?;

        let result = stmt
            .query_row(params![txid, vout], |row| {
                let created_at: String = row.get(8)?;
                let created_at_parsed = DateTime::parse_from_rfc3339(&created_at)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?
                    .with_timezone(&Utc);

                Ok(UtxoUpdate {
                    id: row.get(0)?,
                    address: row.get(1)?,
                    public_key: row.get(2)?,
                    txid: row.get(3)?,
                    vout: row.get(4)?,
                    amount: row.get(5)?,
                    script_pub_key: row.get(6)?,
                    script_type: row.get(7)?,
                    created_at: created_at_parsed,
                    block_height: row.get(9)?,
                    spent_txid: row.get(10)?,
                    spent_at: row.get::<_, Option<String>>(11)?.map(|s| {
                        DateTime::parse_from_rfc3339(&s)
                            .unwrap()
                            .with_timezone(&Utc)
                    }),
                    spent_block: row.get(12)?,
                })
            })
            .optional()
            .map_err(|e| StorageError::DatabaseQueryFailed(e.to_string()))?;

        Ok(result)
    }
}
