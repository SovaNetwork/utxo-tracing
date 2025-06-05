use std::sync::Arc;

use chrono::Utc;
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::params;
use rusqlite_migration::{Migrations, M};

use crate::error::{StorageError, StorageResult};

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
        let migrations = Migrations::new(vec![M::up(
            "CREATE TABLE IF NOT EXISTS signed_transactions (\n                id INTEGER PRIMARY KEY AUTOINCREMENT,\n                txid TEXT NOT NULL,\n                signed_tx TEXT NOT NULL,\n                caller TEXT NOT NULL,\n                timestamp TEXT NOT NULL\n            )",
        )]);

        let mut conn = self
            .conn
            .get()
            .map_err(|e| StorageError::DatabaseConnectionFailed(e.to_string()))?;
        migrations
            .to_latest(&mut conn)
            .map_err(|e| StorageError::MigrationFailed(e.to_string()))?;
        Ok(())
    }

    pub fn store_signed_tx(&self, txid: &str, signed_tx: &str, caller: &str) -> StorageResult<()> {
        let conn = self
            .conn
            .get()
            .map_err(|e| StorageError::DatabaseConnectionFailed(e.to_string()))?;
        conn.execute(
            "INSERT INTO signed_transactions (txid, signed_tx, caller, timestamp) VALUES (?1, ?2, ?3, ?4)",
            params![txid, signed_tx, caller, Utc::now().to_rfc3339()],
        )
        .map_err(|e| StorageError::DatabaseQueryFailed(e.to_string()))?;
        Ok(())
    }
}
