use std::{collections::HashMap, fs, io, path::PathBuf, sync::Arc};

use csv::Reader;
use parking_lot::RwLock;
use tracing::error;

use network_shared::UtxoUpdate;

use super::Datasource;

use crate::error::{StorageError, StorageResult};
use crate::models::block::BlockRow;
use crate::models::utxo::{PendingChanges, UtxoRow};

/// UtxoCSVDatasource
/// - utxos: btc_address -> HashMap<utxo_id, UtxoUpdate> (current UTXO set)
/// - blocks: block_height -> HashMap<btc_address, Vec<UtxoUpdate>> (UTXOs created/spent in this block)
/// - latest_block: latest processed block height
/// - data_dir: data directory
#[derive(Default)]
pub struct UtxoCSVDatasource {
    utxos: RwLock<HashMap<String, HashMap<String, UtxoUpdate>>>,
    blocks: RwLock<HashMap<i32, HashMap<String, Vec<UtxoUpdate>>>>,
    latest_block: RwLock<i32>,
    data_dir: PathBuf,
}

impl UtxoCSVDatasource {
    pub fn new() -> Arc<Self> {
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
    fn setup(&self) -> StorageResult<()> {
        // No setup required
        Ok(())
    }

    fn get_latest_block(&self) -> StorageResult<i32> {
        let value = self.latest_block.read();
        Ok(*value)
    }

    fn get_type(&self) -> String {
        String::from("CSV")
    }

    fn process_block_utxos(&self, changes: &PendingChanges) -> StorageResult<()> {
        // Validate parameters
        if changes.height < 0 {
            return Err(StorageError::InvalidBlockHeight(changes.height));
        }

        let mut utxos = self.utxos.write();
        let mut block_utxos: HashMap<String, Vec<UtxoUpdate>> = HashMap::new();

        // New UTXOs we need to add to an address' set
        for utxo in &changes.utxos_insert {
            // Validate UTXO
            if utxo.amount < 0 {
                return Err(StorageError::InvalidAmount(utxo.amount));
            }

            if utxo.address.is_empty() {
                return Err(StorageError::InvalidAddress("Empty address".to_string()));
            }

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
            .insert(changes.height, block_utxos.clone());

        // Existing UTXOs we need to update
        for utxo in &changes.utxos_update {
            // Validate UTXO
            if utxo.amount < 0 {
                return Err(StorageError::InvalidAmount(utxo.amount));
            }

            if utxo.address.is_empty() {
                return Err(StorageError::InvalidAddress("Empty address".to_string()));
            }

            let utxo_id = format!("{}:{}", utxo.txid, utxo.vout);

            if let Some(address_utxos) = utxos.get_mut(&utxo.address) {
                if let Some(existing_utxo) = address_utxos.get_mut(&utxo_id) {
                    existing_utxo.spent_txid = utxo.spent_txid.clone();
                    existing_utxo.spent_at = utxo.spent_at.clone();
                    existing_utxo.spent_block = utxo.spent_block.clone();
                }
            }
        }

        *self.latest_block.write() = changes.height;

        // TODO: Implement save_changes to csv
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

        let utxos = self.utxos.read();

        if let Some(address_utxos) = utxos.get(address) {
            // Filter UTXOs that:
            // 1. Were created at or before this block height
            // 2. Either haven't been spent, or were spent after this block height
            let result = address_utxos
                .values()
                .filter(|utxo| {
                    utxo.block_height <= block_height && // Created at or before this height
                    match utxo.spent_block {
                        None => true, // Not spent
                        Some(spent_height) => spent_height > block_height // Spent after this height
                    }
                })
                .cloned()
                .collect();

            Ok(result)
        } else {
            Ok(Vec::new())
        }
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

        let result = self
            .blocks
            .read()
            .get(&block_height)
            .and_then(|block_data| block_data.get(address))
            .map(|utxos| utxos.clone())
            .unwrap_or_default();

        Ok(result)
    }

    fn get_all_utxos_for_block(&self, block_height: i32) -> StorageResult<Vec<UtxoUpdate>> {
        // Validate parameters
        if block_height < 0 {
            return Err(StorageError::InvalidBlockHeight(block_height));
        }

        let blocks = self.blocks.read();
        let result = blocks
            .get(&block_height)
            .map(|block_data| {
                block_data
                    .values()
                    .flat_map(|utxos| utxos.iter().cloned())
                    .collect()
            })
            .unwrap_or_default();

        Ok(result)
    }
}
