use chrono::{DateTime, Utc};
use network_shared::UtxoUpdate;
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::params;
use rusqlite_migration::{Migrations, M};
use std::sync::Arc;

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

        std::fs::create_dir_all(&data_dir)
            .map_err(|e| StorageError::IoError(e.to_string()))?;
            
        let db_dir = data_dir.join("utxo.db");
        let db_path = db_dir.to_str()
            .ok_or_else(|| StorageError::UnexpectedError("Invalid database path".to_string()))?;

        let manager = SqliteConnectionManager::file(db_path);
        let conn = Pool::new(manager)
            .map_err(|e| StorageError::DatabaseConnectionFailed(e.to_string()))?;

        Ok(Arc::new(Self { conn }))
    }
    
    fn run_migrations(&self) -> StorageResult<()> {
        let migrations = Migrations::new(vec![M::up(
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
                created_at TEXT NOT NULL,      -- ISO 8061 format for timestamptz
                block_height INTEGER NOT NULL,
                spent_txid TEXT,
                spent_at TEXT,                 -- ISO 8061 format for timestamptz
                spent_block INTEGER,
                UNIQUE(txid, vout)             -- Ensure (txid, vout) is unique
            )",
        )]);

        let mut conn = self.conn.get()
            .map_err(|e| StorageError::DatabaseConnectionFailed(e.to_string()))?;

        migrations.to_latest(&mut conn)
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
        ).map_err(|e| StorageError::DatabaseQueryFailed(e.to_string()))?;

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
        let conn = self.conn.get()
            .map_err(|e| StorageError::DatabaseConnectionFailed(e.to_string()))?;

        let query = "
            SELECT MAX(
                CASE
                    WHEN spent_block IS NOT NULL AND spent_block > block_height THEN spent_block
                    ELSE block_height
                END
            ) AS latest_block
            FROM utxo;
        ";

        conn.query_row(query, [], |row| row.get(0))
            .map_err(|e| StorageError::DatabaseQueryFailed(e.to_string()))
    }

    fn process_block_utxos(&self, changes: &PendingChanges) -> StorageResult<()> {
        // Validate parameters
        if changes.height < 0 {
            return Err(StorageError::InvalidBlockHeight(changes.height));
        }
        
        let mut conn = self.conn.get()
            .map_err(|e| StorageError::DatabaseConnectionFailed(e.to_string()))?;

        // Start a transaction
        let tx = conn.transaction()
            .map_err(|e| StorageError::DatabaseQueryFailed(format!("Failed to start transaction: {}", e)))?;

        // Upsert UTXOs from utxos_update
        for utxo in &changes.utxos_update {
            if let Err(e) = UtxoSqliteDatasource::upsert_utxo_in_tx(&tx, utxo) {
                let _ = tx.rollback(); // Try to rollback, but we'll return the original error
                return Err(StorageError::DatabaseQueryFailed(
                    format!("Failed to upsert UTXO {}: {}", utxo.id, e)
                ));
            }
        }

        // Upsert UTXOs from utxos_insert
        for utxo in &changes.utxos_insert {
            if let Err(e) = UtxoSqliteDatasource::upsert_utxo_in_tx(&tx, utxo) {
                let _ = tx.rollback(); // Try to rollback, but we'll return the original error
                return Err(StorageError::DatabaseQueryFailed(
                    format!("Failed to upsert UTXO {}: {}", utxo.id, e)
                ));
            }
        }

        // Commit the transaction
        tx.commit().map_err(|e| StorageError::DatabaseQueryFailed(
            format!("Failed to commit transaction: {}", e)
        ))?;

        Ok(())
    }

    fn get_spendable_utxos_at_height(&self, block_height: i32, address: &str) -> StorageResult<Vec<UtxoUpdate>> {
        // Validate parameters
        if block_height < 0 {
            return Err(StorageError::InvalidBlockHeight(block_height));
        }
        
        if address.is_empty() {
            return Err(StorageError::InvalidAddress("Empty address".to_string()));
        }

        let conn = self.conn.get()
            .map_err(|e| StorageError::DatabaseConnectionFailed(e.to_string()))?;

        let mut stmt = conn.prepare(
        "SELECT 
            id, address, public_key, txid, vout, amount, script_pub_key, 
            script_type, created_at, block_height, spent_txid, spent_at, spent_block
            FROM utxo
            WHERE block_height <= ?1 
            AND (spent_block IS NULL OR spent_block > ?1)
            AND address = ?2",
        ).map_err(|e| StorageError::DatabaseQueryFailed(format!("Failed to prepare statement: {}", e)))?;

        let mut results = Vec::new();

        // Use params! to ensure the types of block_height and address match the placeholders
        let rows = stmt.query_map(params![block_height, address], |row| {
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
        }).map_err(|e| StorageError::DatabaseQueryFailed(format!("Failed to query rows: {}", e)))?;

        for row in rows {
            match row {
                Ok(utxo) => results.push(utxo),
                Err(e) => return Err(StorageError::DatabaseQueryFailed(format!("Failed to process row: {}", e))),
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

        let conn = self.conn.get()
            .map_err(|e| StorageError::DatabaseConnectionFailed(e.to_string()))?;
            
        let query = "
            SELECT 
                id, address, public_key, txid, vout, amount, script_pub_key, 
                script_type, created_at, block_height, spent_txid, spent_at, spent_block
            FROM utxo
            WHERE block_height = ?1 AND address = ?2;
        ";

        // Prepare the statement
        let mut stmt = conn.prepare(query)
            .map_err(|e| StorageError::DatabaseQueryFailed(format!("Failed to prepare statement: {}", e)))?;

        // Execute the query and map results to UtxoUpdate
        let rows = stmt.query_map(params![block_height, address], |row| {
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
        }).map_err(|e| StorageError::DatabaseQueryFailed(format!("Failed to query rows: {}", e)))?;

        // Collect the results into a vector
        let mut results = Vec::new();
        for row in rows {
            match row {
                Ok(utxo) => results.push(utxo),
                Err(e) => return Err(StorageError::DatabaseQueryFailed(format!("Failed to process row: {}", e))),
            }
        }

        Ok(results)
    }

    fn get_all_utxos_for_block(&self, block_height: i32) -> StorageResult<Vec<UtxoUpdate>> {
        // Validate parameters
        if block_height < 0 {
            return Err(StorageError::InvalidBlockHeight(block_height));
        }

        let conn = self.conn.get()
            .map_err(|e| StorageError::DatabaseConnectionFailed(e.to_string()))?;
            
        let query = "
            SELECT 
                id, address, public_key, txid, vout, amount, script_pub_key, 
                script_type, created_at, block_height, spent_txid, spent_at, spent_block
            FROM utxo
            WHERE block_height = ?1;
        ";

        let mut stmt = conn.prepare(query)
            .map_err(|e| StorageError::DatabaseQueryFailed(format!("Failed to prepare statement: {}", e)))?;

        let rows = stmt.query_map([block_height], |row| {
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
        }).map_err(|e| StorageError::DatabaseQueryFailed(format!("Failed to query rows: {}", e)))?;

        let mut results = Vec::new();
        for row in rows {
            match row {
                Ok(utxo) => results.push(utxo),
                Err(e) => return Err(StorageError::DatabaseQueryFailed(format!("Failed to process row: {}", e))),
            }
        }

        Ok(results)
    }
}