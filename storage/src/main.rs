use std::{
    collections::{HashMap, HashSet},
    fs, io,
    path::PathBuf,
    sync::Arc,
};

use actix_web::{middleware::Logger, web, App, HttpResponse, HttpServer};
use chrono::{DateTime, Utc};
use clap::Parser;
use csv::Reader;
use parking_lot::RwLock;
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{params, Result};
use rusqlite_migration::{Migrations, M};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use tokio::net::{UnixListener, UnixStream};
use tracing::{error, info, instrument};

use network_shared::{BlockUpdate, UtxoUpdate};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Host address to bind to
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    /// Port to listen on
    #[arg(long, default_value = "5557")]
    port: u16,

    /// Log level
    #[arg(long, default_value = "info")]
    log_level: String,

    /// Datasource type
    #[arg(long, default_value = "csv")]
    datasource: String,

    /// Socket path for receiving updates
    #[arg(long, default_value = "/tmp/network-utxos.sock")]
    socket_path: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct UtxoRow {
    // Key fields for storage
    address: String,
    utxo_id: String,

    // UTXO data fields
    id: String,
    utxo_address: String,
    public_key: Option<String>,
    txid: String,
    vout: i32,
    amount: i64,
    script_pub_key: String,
    script_type: String,
    created_at: DateTime<Utc>,
    block_height: i32,
    spent_txid: Option<String>,
    spent_at: Option<DateTime<Utc>>,
    spent_block: Option<i32>,
}

impl UtxoRow {
    fn into_storage_entry(self) -> (String, String, UtxoUpdate) {
        let utxo = UtxoUpdate {
            id: self.id,
            address: self.utxo_address,
            public_key: self.public_key,
            txid: self.txid,
            vout: self.vout,
            amount: self.amount,
            script_pub_key: self.script_pub_key,
            script_type: self.script_type,
            created_at: self.created_at,
            block_height: self.block_height,
            spent_txid: self.spent_txid,
            spent_at: self.spent_at,
            spent_block: self.spent_block,
        };
        (self.address, self.utxo_id, utxo)
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct BlockRow {
    // Key fields
    height: i32,
    address: String,

    // UTXO data fields
    id: String,
    utxo_address: String,
    public_key: Option<String>,
    txid: String,
    vout: i32,
    amount: i64,
    script_pub_key: String,
    script_type: String,
    created_at: DateTime<Utc>,
    block_height: i32,
    spent_txid: Option<String>,
    spent_at: Option<DateTime<Utc>>,
    spent_block: Option<i32>,
}

impl BlockRow {
    fn into_utxo(self) -> UtxoUpdate {
        UtxoUpdate {
            id: self.id,
            address: self.utxo_address,
            public_key: self.public_key,
            txid: self.txid,
            vout: self.vout,
            amount: self.amount,
            script_pub_key: self.script_pub_key,
            script_type: self.script_type,
            created_at: self.created_at,
            block_height: self.block_height,
            spent_txid: self.spent_txid,
            spent_at: self.spent_at,
            spent_block: self.spent_block,
        }
    }
}

struct PendingChanges {
    height: i32,
    utxos_update: Vec<UtxoUpdate>, // modified UTXOs
    utxos_insert: Vec<UtxoUpdate>, // new UTXOs
}

trait Datasource {
    fn setup(&self);
    fn get_type(&self) -> String;
    fn get_latest_block(&self) -> i32;
    fn process_block_utxos(&self, pending_changes: &PendingChanges);
    fn get_spendable_utxos_at_height(&self, block_height: i32, address: &str) -> Vec<UtxoUpdate>;
    fn get_utxos_for_block_and_address(
        &self,
        block_height: i32,
        address: &str,
    ) -> Option<Vec<UtxoUpdate>>;
    fn get_all_utxos_for_block(&self, block_height: i32) -> Option<Vec<UtxoUpdate>>;
}

/// UtxoCSVDatasource
/// - utxos: btc_address -> HashMap<utxo_id, UtxoUpdate> (current UTXO set)
/// - blocks: block_height -> HashMap<btc_address, Vec<UtxoUpdate>> (UTXOs created/spent in this block)
/// - latest_block: latest processed block height
/// - data_dir: data directory
#[derive(Default)]
struct UtxoCSVDatasource {
    utxos: RwLock<HashMap<String, HashMap<String, UtxoUpdate>>>,
    blocks: RwLock<HashMap<i32, HashMap<String, Vec<UtxoUpdate>>>>,
    latest_block: RwLock<i32>,
    data_dir: PathBuf,
}

impl UtxoCSVDatasource {
    fn new() -> Arc<Self> {
        // Create data directory if it doesn't exist
        let data_dir = std::env::current_dir().unwrap().join("data");
        fs::create_dir_all(&data_dir).expect("Failed to create data directory");

        let db = Arc::new(Self {
            utxos: Default::default(),
            blocks: Default::default(),
            latest_block: Default::default(),
            data_dir,
        });

        // Load existing data if available
        if let Err(e) = db.load_data() {
            error!("Failed to load existing data: {}", e);
        }

        db
    }

    fn get_utxo_file_path(&self) -> PathBuf {
        self.data_dir.join("utxos.csv")
    }

    fn get_block_file_path(&self) -> PathBuf {
        self.data_dir.join("blocks.csv")
    }

    fn load_data(&self) -> io::Result<()> {
        self.load_utxos()?;
        self.load_blocks()?;
        Ok(())
    }

    fn load_utxos(&self) -> io::Result<()> {
        let path = self.get_utxo_file_path();
        if !path.exists() {
            return Ok(());
        }

        let mut reader = Reader::from_path(&path)?;
        let mut utxos = self.utxos.write();

        for result in reader.deserialize() {
            let row: UtxoRow = result?;
            let (address, utxo_id, utxo) = row.into_storage_entry();

            utxos.entry(address).or_default().insert(utxo_id, utxo);
        }

        Ok(())
    }

    fn load_blocks(&self) -> io::Result<()> {
        let path = self.get_block_file_path();
        if !path.exists() {
            return Ok(());
        }

        let mut reader = Reader::from_path(&path)?;
        let mut blocks = self.blocks.write();
        let mut latest_height = 0;

        for result in reader.deserialize() {
            let row: BlockRow = result?;
            let row_clone = row.clone();

            blocks
                .entry(row.height)
                .or_default()
                .entry(row.address)
                .or_default()
                .push(row_clone.into_utxo());

            latest_height = latest_height.max(row.height);
        }

        if latest_height > 0 {
            *self.latest_block.write() = latest_height;
        }

        Ok(())
    }
}

impl Datasource for UtxoCSVDatasource {
    fn setup(&self) {
        // No setup required
        return;
    }

    fn get_latest_block(&self) -> i32 {
        let value = self.latest_block.read();
        *value
    }

    fn get_type(&self) -> String {
        String::from("CSV")
    }

    fn process_block_utxos(&self, changes: &PendingChanges) {
        let mut utxos = self.utxos.write();
        let mut block_utxos: HashMap<String, Vec<UtxoUpdate>> = HashMap::new();

        // New UTXOs we need to add to an address' set
        for utxo in &changes.utxos_insert {
            let utxo_id = format!("{}:{}", utxo.txid, utxo.vout);

            utxos
                .entry(utxo.address.clone())
                .or_default()
                .insert(utxo_id, utxo.clone());

            block_utxos
                .entry(utxo.address.clone())
                .or_default()
                .push(utxo.clone())
        }

        // Add new block utxos
        self.blocks
            .write()
            .insert(changes.height.clone(), block_utxos.clone());

        // Existing UTXOs we need to update
        for utxo in &changes.utxos_update {
            let utxo_id = format!("{}:{}", utxo.txid, utxo.vout);

            if let Some(address_utxos) = utxos.get_mut(&utxo.address) {
                if let Some(existing_utxo) = address_utxos.get_mut(&utxo_id) {
                    existing_utxo.spent_txid = utxo.spent_txid.clone();
                    existing_utxo.spent_at = utxo.spent_at.clone();
                    existing_utxo.spent_block = utxo.spent_block.clone();
                }
            }
        }

        *self.latest_block.write() = changes.height.clone();

        // save_changes to csv
    }

    fn get_spendable_utxos_at_height(&self, block_height: i32, address: &str) -> Vec<UtxoUpdate> {
        let utxos = self.utxos.read();

        if let Some(address_utxos) = utxos.get(address) {
            // Filter UTXOs that:
            // 1. Were created at or before this block height
            // 2. Either haven't been spent, or were spent after this block height
            address_utxos
                .values()
                .filter(|utxo| {
                    utxo.block_height <= block_height && // Created at or before this height
                    match utxo.spent_block {
                        None => true, // Not spent
                        Some(spent_height) => spent_height > block_height // Spent after this height
                    }
                })
                .cloned()
                .collect()
        } else {
            Vec::new()
        }
    }

    fn get_utxos_for_block_and_address(
        &self,
        block_height: i32,
        address: &str,
    ) -> Option<Vec<UtxoUpdate>> {
        self.blocks
            .read()
            .get(&block_height)
            .and_then(|block_data| block_data.get(address))
            .map(|utxos| utxos.clone())
    }

    fn get_all_utxos_for_block(&self, block_height: i32) -> Option<Vec<UtxoUpdate>> {
        let blocks = self.blocks.read();
        blocks.get(&block_height).map(|block_data| {
            block_data
                .values()
                .flat_map(|utxos| utxos.iter().cloned())
                .collect()
        })
    }
}

struct UtxoSqliteDatasource {
    conn: Pool<SqliteConnectionManager>,
}

impl UtxoSqliteDatasource {
    fn new() -> Arc<Self> {
        let data_dir = std::env::current_dir().unwrap().join("data");
        fs::create_dir_all(&data_dir).expect("Failed to create data directory");
        let db_dir = data_dir.join("utxo.db");
        let db_path = db_dir.to_str().unwrap();

        let manager = SqliteConnectionManager::file(db_path);
        let conn = Pool::new(manager).unwrap();

        return Arc::new(Self { conn: conn });
    }

    fn run_migrations(&self) -> Result<()> {
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

        let mut conn = self.conn.get().unwrap();

        let _ = migrations.to_latest(&mut conn);

        Ok(())
    }

    fn upsert_utxo_in_tx(tx: &rusqlite::Transaction, utxo: &UtxoUpdate) -> rusqlite::Result<()> {
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
        )?;

        Ok(())
    }
}

impl Datasource for UtxoSqliteDatasource {
    fn setup(&self) {
        let _ = self.run_migrations();

        return;
    }

    fn get_type(&self) -> String {
        String::from("Sqlite")
    }

    fn get_latest_block(&self) -> i32 {
        let conn = self.conn.get().unwrap();
        let query = "
            SELECT MAX(
                CASE
                    WHEN spent_block IS NOT NULL AND spent_block > block_height THEN spent_block
                    ELSE block_height
                END
            ) AS latest_block
            FROM utxo;
        ";

        match conn.query_row(query, [], |row| row.get(0)) {
            Ok(latest_block) => latest_block,
            Err(e) => {
                eprintln!("Failed to query the latest block: {}", e);
                0 // Return 0 in case of an error
            }
        }
    }

    fn process_block_utxos(&self, changes: &PendingChanges) {
        let mut conn = self.conn.get().unwrap();

        // Start a transaction
        let tx = match conn.transaction() {
            Ok(tx) => tx,
            Err(e) => {
                eprintln!("Failed to start transaction: {}", e);
                return;
            }
        };

        // Upsert UTXOs from utxos_update
        for utxo in &changes.utxos_update {
            if let Err(e) = UtxoSqliteDatasource::upsert_utxo_in_tx(&tx, utxo) {
                eprintln!("Failed to upsert UTXO {}: {}", utxo.id, e);
                tx.rollback().expect("Failed to rollback transaction");
                return;
            }
        }

        // Upsert UTXOs from utxos_insert
        for utxo in &changes.utxos_insert {
            if let Err(e) = UtxoSqliteDatasource::upsert_utxo_in_tx(&tx, utxo) {
                eprintln!("Failed to upsert UTXO {}: {}", utxo.id, e);
                tx.rollback().expect("Failed to rollback transaction");
                return;
            }
        }

        // Commit the transaction
        if let Err(e) = tx.commit() {
            eprintln!("Failed to commit transaction: {}", e);
        } else {
            println!("Processed block UTXOs for height {}", changes.height);
        }
    }

    fn get_spendable_utxos_at_height(&self, block_height: i32, address: &str) -> Vec<UtxoUpdate> {
        let conn = self.conn.get().unwrap();

        let mut stmt = match conn.prepare(
            "SELECT 
                id, address, public_key, txid, vout, amount, script_pub_key, 
                script_type, created_at, block_height, spent_txid, spent_at, spent_block
             FROM utxo
             WHERE block_height <= ?1 
             AND (spent_block IS NULL OR spent_block > ?1)
             AND address = ?2",
        ) {
            Ok(stmt) => stmt,
            Err(e) => {
                eprintln!("Failed to prepare statement: {}", e);
                return Vec::new();
            }
        };

        let mut results = Vec::new();

        // Use params! to ensure the types of block_height and address match the placeholders
        let rows = match stmt.query_map(params![block_height, address], |row| {
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
        }) {
            Ok(rows) => rows,
            Err(e) => {
                eprintln!("Failed to query rows: {}", e);
                return Vec::new();
            }
        };

        for row in rows {
            match row {
                Ok(utxo) => results.push(utxo),
                Err(e) => eprintln!("Failed to process row: {}", e),
            }
        }

        results
    }

    fn get_utxos_for_block_and_address(
        &self,
        block_height: i32,
        address: &str,
    ) -> Option<Vec<UtxoUpdate>> {
        let conn = self.conn.get().unwrap();
        let query = "
            SELECT 
                id, address, public_key, txid, vout, amount, script_pub_key, 
                script_type, created_at, block_height, spent_txid, spent_at, spent_block
            FROM utxo
            WHERE block_height = ?1 AND address = ?2;
        ";

        // Prepare the statement
        let mut stmt = match conn.prepare(query) {
            Ok(stmt) => stmt,
            Err(e) => {
                eprintln!("Failed to prepare statement: {}", e);
                return None;
            }
        };

        // Execute the query and map results to UtxoUpdate
        let rows = match stmt.query_map(params![block_height, address], |row| {
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
        }) {
            Ok(rows) => rows,
            Err(e) => {
                eprintln!("Failed to query rows: {}", e);
                return None;
            }
        };

        // Collect the results into a vector
        let mut results = Vec::new();
        for row in rows {
            match row {
                Ok(utxo) => results.push(utxo),
                Err(e) => eprintln!("Failed to process row: {}", e),
            }
        }

        // Return the results if not empty, otherwise return None
        if results.is_empty() {
            None
        } else {
            Some(results)
        }
    }

    fn get_all_utxos_for_block(&self, block_height: i32) -> Option<Vec<UtxoUpdate>> {
        let conn = self.conn.get().unwrap();
        let query = "
            SELECT 
                id, address, public_key, txid, vout, amount, script_pub_key, 
                script_type, created_at, block_height, spent_txid, spent_at, spent_block
            FROM utxo
            WHERE block_height = ?1;
        ";

        let mut stmt = match conn.prepare(query) {
            Ok(stmt) => stmt,
            Err(e) => {
                eprintln!("Failed to prepare statement: {}", e);
                return None;
            }
        };

        let rows = match stmt.query_map([block_height], |row| {
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
        }) {
            Ok(rows) => rows,
            Err(e) => {
                eprintln!("Failed to query rows: {}", e);
                return None;
            }
        };

        let mut results = Vec::new();
        for row in rows {
            match row {
                Ok(utxo) => results.push(utxo),
                Err(e) => eprintln!("Failed to process row: {}", e),
            }
        }

        if results.is_empty() {
            None
        } else {
            Some(results)
        }
    }
}

struct UtxoDatabase {
    datasource: Arc<dyn Datasource + Send + Sync>, // Send + Sync to make Arc thread safe
}

impl UtxoDatabase {
    fn new(datasource: Arc<dyn Datasource + Send + Sync>) -> Arc<Self> {
        info!(
            "Initializing UTXO database with storage type: {}",
            datasource.get_type()
        );

        return Arc::new(Self { datasource });
    }

    fn get_latest_block(&self) -> i32 {
        return self.datasource.get_latest_block();
    }

    #[instrument(skip(self, block), fields(block_height = block.height))]
    async fn process_block(&self, block: BlockUpdate) -> Result<(), String> {
        let height = block.height;
        info!(height, "Processing new block");

        let mut pending_changes = PendingChanges {
            height: height,
            utxos_update: Vec::new(),
            utxos_insert: Vec::new(),
        };

        // Process UTXO updates
        for utxo in block.utxo_updates {
            // Handle spent UTXOs first
            if utxo.spent_txid.is_some() {
                pending_changes.utxos_update.push(utxo.clone());
            } else {
                // This is a new UTXO being created, track for saving
                pending_changes.utxos_insert.push(utxo.clone());
            }
        }

        self.datasource.process_block_utxos(&pending_changes);

        info!(height, "Block processing completed");
        Ok(())
    }

    fn get_spendable_utxos_at_height(&self, block_height: i32, address: &str) -> Vec<UtxoUpdate> {
        return self
            .datasource
            .get_spendable_utxos_at_height(block_height, address);
    }

    fn select_utxos_for_amount(
        &self,
        block_height: i32,
        address: &str,
        target_amount: i64,
    ) -> Vec<UtxoUpdate> {
        let mut spendable_utxos = self.get_spendable_utxos_at_height(block_height, address);

        // Sort by block height (FIFO) - earlier blocks first
        spendable_utxos.sort_by_key(|utxo| utxo.block_height);

        let mut selected_utxos = Vec::new();
        let mut accumulated_amount = 0;

        for utxo in spendable_utxos {
            if accumulated_amount >= target_amount {
                break;
            }

            selected_utxos.push(utxo.clone());
            accumulated_amount += utxo.amount;
        }

        // Only return UTXOs if we have enough to meet the target amount
        if accumulated_amount >= target_amount {
            selected_utxos
        } else {
            Vec::new()
        }
    }

    fn get_utxos_for_block_and_address(
        &self,
        block_height: i32,
        address: &str,
    ) -> Option<Vec<UtxoUpdate>> {
        return self
            .datasource
            .get_utxos_for_block_and_address(block_height, address);
    }

    fn get_block_txids(&self, block_height: i32) -> Vec<String> {
        let mut txids = HashSet::new();

        // Get all UTXOs for this block height from the datasource
        if let Some(utxos) = self.datasource.get_all_utxos_for_block(block_height) {
            // Collect txids from UTXOs created in this block
            for utxo in utxos {
                txids.insert(utxo.txid.clone());
                // Also include spending transactions that happened in this block
                if let Some(spent_txid) = utxo.spent_txid {
                    if utxo.spent_block == Some(block_height) {
                        txids.insert(spent_txid);
                    }
                }
            }
        }

        txids.into_iter().collect()
    }
}

// Handle incoming Unix socket connections
async fn handle_socket_connection(stream: UnixStream, db: Arc<UtxoDatabase>) -> io::Result<()> {
    let mut stream = stream;

    // Read size header
    let mut size_buf = [0u8; 4];
    stream.read_exact(&mut size_buf).await?;
    let size = u32::from_le_bytes(size_buf) as usize;

    // Read data
    let mut data = vec![0u8; size];
    stream.read_exact(&mut data).await?;

    // Deserialize
    let update: BlockUpdate = bincode::deserialize(&data)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

    info!(height = update.height, "Received block update via socket");

    // Process the update
    if let Err(e) = db.process_block(update).await {
        error!("Error processing block: {}", e);
    }

    Ok(())
}

// Socket server that listens for Unix socket connections
async fn run_socket_server(db: Arc<UtxoDatabase>, socket_path: &str) -> io::Result<()> {
    // Remove socket file if it exists
    let socket_path = std::path::Path::new(socket_path);
    if socket_path.exists() {
        fs::remove_file(socket_path)?;
    }

    // Create Unix socket listener
    let listener = UnixListener::bind(socket_path)?;
    info!("Socket server listening on {}", socket_path.display());

    // Accept connections
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let db_clone = db.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_socket_connection(stream, db_clone).await {
                        error!("Error handling socket connection: {}", e);
                    }
                });
            }
            Err(e) => {
                error!("Error accepting connection: {}", e);
            }
        }
    }
}

#[derive(Clone)]
struct AppState {
    db: Arc<UtxoDatabase>,
}

// HTTP handlers
#[instrument(skip(state))]
async fn get_latest_block(state: web::Data<AppState>) -> HttpResponse {
    info!("Getting latest block height");

    let latest_block = state.db.get_latest_block();

    let response = serde_json::json!({
        "latest_block": latest_block
    });

    HttpResponse::Ok().json(response)
}

#[instrument(skip(state))]
async fn get_block_txids(state: web::Data<AppState>, path: web::Path<i32>) -> HttpResponse {
    let block_height = path.into_inner();

    info!(block_height, "Getting transaction IDs for block");

    let latest_block = state.db.get_latest_block();
    if block_height > latest_block {
        return HttpResponse::NotFound().json(serde_json::json!({
            "error": "Block not found",
            "latest_block": latest_block
        }));
    }

    let txids = state.db.get_block_txids(block_height);

    let response = serde_json::json!({
        "txids": txids
    });

    HttpResponse::Ok().json(response)
}

#[instrument(skip(state))]
async fn get_block_address_utxos(
    state: web::Data<AppState>,
    path: web::Path<(i32, String)>,
) -> HttpResponse {
    let (block_height, address) = path.into_inner();

    info!(block_height, %address, "Querying UTXOs for block and address");

    let latest_block = state.db.get_latest_block();
    if block_height > latest_block {
        return HttpResponse::NotFound().json(serde_json::json!({
            "error": "Block not found",
            "latest_block": latest_block
        }));
    }

    match state
        .db
        .get_utxos_for_block_and_address(block_height, &address)
    {
        Some(utxos) => {
            let response = serde_json::json!({
                "block_height": block_height,
                "address": address,
                "utxos": utxos
            });
            HttpResponse::Ok().json(response)
        }
        None => HttpResponse::NotFound().json(serde_json::json!({
            "error": "No UTXOs found for specified address in the specified block",
        })),
    }
}

#[instrument(skip(state))]
async fn get_spendable_utxos(
    state: web::Data<AppState>,
    path: web::Path<(i32, String)>,
) -> HttpResponse {
    let (block_height, address) = path.into_inner();

    info!(block_height, %address, "Querying spendable UTXOs for address at height");

    let latest_block = state.db.get_latest_block();
    if block_height > latest_block {
        return HttpResponse::NotFound().json(serde_json::json!({
            "error": "Block not found",
            "latest_block": latest_block
        }));
    }

    let spendable_utxos = state
        .db
        .get_spendable_utxos_at_height(block_height, &address);
    let total_amount: i64 = spendable_utxos.iter().map(|utxo| utxo.amount).sum();

    let response = serde_json::json!({
        "block_height": block_height,
        "address": address,
        "spendable_utxos": spendable_utxos,
        "total_amount": total_amount
    });

    HttpResponse::Ok().json(response)
}

#[instrument(skip(state))]
async fn select_utxos(
    state: web::Data<AppState>,
    path: web::Path<(i32, String, i64)>, // block_height, address, target_amount (satoshis)
) -> HttpResponse {
    let (block_height, address, target_amount) = path.into_inner();

    info!(
        block_height,
        %address,
        target_amount,
        "Selecting UTXOs for amount using FIFO"
    );

    let latest_block = state.db.get_latest_block();
    if block_height > latest_block {
        return HttpResponse::NotFound().json(serde_json::json!({
            "error": "Block not found",
            "latest_block": latest_block
        }));
    }

    if target_amount <= 0 {
        return HttpResponse::BadRequest().json(serde_json::json!({
            "error": "Target amount must be greater than 0"
        }));
    }

    let selected_utxos = state
        .db
        .select_utxos_for_amount(block_height, &address, target_amount);
    let total_amount: i64 = selected_utxos.iter().map(|utxo| utxo.amount).sum();

    if selected_utxos.is_empty() {
        return HttpResponse::NotFound().json(serde_json::json!({
            "error": "Insufficient funds to meet target amount",
            "available_amount": total_amount,
            "target_amount": target_amount
        }));
    }

    let response = serde_json::json!({
        "block_height": block_height,
        "address": address,
        "target_amount": target_amount,
        "selected_utxos": selected_utxos,
        "total_amount": total_amount
    });

    HttpResponse::Ok().json(response)
}

fn create_datasource(arg: &str) -> Arc<dyn Datasource + Send + Sync> {
    match arg {
        "csv" => UtxoCSVDatasource::new(),
        "sqlite" => UtxoSqliteDatasource::new(),
        _ => panic!(
            "Invalid argument for datasource: {}, Use 'csv' or 'sqlite'",
            arg
        ),
    }
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let args = Args::parse();

    // Setup tracing
    tracing_subscriber::fmt()
        .with_env_filter(&args.log_level)
        .init();

    info!("Starting UTXO tracking service");

    // Create datasource based on arguments
    let datasource = create_datasource(&args.datasource);
    datasource.setup();

    // Create database
    let db = UtxoDatabase::new(datasource);
    let state = web::Data::new(AppState { db: db.clone() });

    // Run Unix socket server in the background
    let socket_path = args.socket_path.clone();
    let db_clone = db.clone();
    tokio::spawn(async move {
        if let Err(e) = run_socket_server(db_clone, &socket_path).await {
            error!("Socket server error: {}", e);
        }
    });

    // Configure and start HTTP server
    let bind_addr = format!("{}:{}", args.host, args.port);
    info!("Starting HTTP server on {}", bind_addr);

    HttpServer::new(move || {
        App::new()
            .wrap(Logger::default())
            .app_data(state.clone())
            // Block info endpoints
            .route("/latest-block", web::get().to(get_latest_block))
            .route("/block/{height}/txids", web::get().to(get_block_txids))
            // UTXO query endpoints
            .route(
                "/utxos/block/{height}/address/{address}",
                web::get().to(get_block_address_utxos),
            )
            .route(
                "/spendable-utxos/block/{height}/address/{address}",
                web::get().to(get_spendable_utxos),
            )
            .route(
                "/select-utxos/block/{height}/address/{address}/amount/{amount}",
                web::get().to(select_utxos),
            )
    })
    .bind(bind_addr)?
    .run()
    .await
}
