use std::collections::HashSet;
use std::sync::Arc;

use tracing::{info, instrument};

use network_shared::{BlockUpdate, UtxoUpdate};

use crate::datasources::Datasource;
use crate::error::{StorageError, StorageResult};
use crate::models::utxo::PendingChanges;

pub struct UtxoDatabase {
    datasource: Arc<dyn Datasource + Send + Sync>, // Send + Sync to make Arc thread safe
}

impl UtxoDatabase {
    pub fn new(datasource: Arc<dyn Datasource + Send + Sync>) -> Arc<Self> {
        info!(
            "Initializing UTXO database with storage type: {}",
            datasource.get_type()
        );

        return Arc::new(Self { datasource });
    }

    pub fn get_latest_block(&self) -> StorageResult<i32> {
        self.datasource.get_latest_block()
    }

    #[instrument(skip(self, block), fields(block_height = block.height))]
    pub async fn process_block(&self, block: BlockUpdate) -> StorageResult<()> {
        let height = block.height;

        // Validate parameters
        if height < 0 {
            return Err(StorageError::InvalidBlockHeight(height));
        }

        info!(height, "Processing new block");

        let mut pending_changes = PendingChanges {
            height,
            utxos_update: Vec::new(),
            utxos_insert: Vec::new(),
        };

        // Process UTXO updates
        for utxo in block.utxo_updates {
            // Validate UTXO
            if utxo.amount < 0 {
                return Err(StorageError::InvalidAmount(utxo.amount));
            }

            if utxo.address.is_empty() {
                return Err(StorageError::InvalidAddress("Empty address".to_string()));
            }

            // Handle spent UTXOs first
            if utxo.spent_txid.is_some() {
                pending_changes.utxos_update.push(utxo.clone());
            } else {
                // This is a new UTXO being created, track for saving
                pending_changes.utxos_insert.push(utxo.clone());
            }
        }

        self.datasource.process_block_utxos(&pending_changes)?;

        info!(height, "Block processing completed");
        Ok(())
    }

    pub fn get_spendable_utxos_at_height(
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

        self.datasource
            .get_spendable_utxos_at_height(block_height, address)
    }

    pub fn select_utxos_for_amount(
        &self,
        block_height: i32,
        address: &str,
        target_amount: i64,
    ) -> StorageResult<Vec<UtxoUpdate>> {
        // Validate parameters
        if block_height < 0 {
            return Err(StorageError::InvalidBlockHeight(block_height));
        }

        if address.is_empty() {
            return Err(StorageError::InvalidAddress("Empty address".to_string()));
        }

        if target_amount <= 0 {
            return Err(StorageError::InvalidAmount(target_amount));
        }

        let spendable_utxos = self.get_spendable_utxos_at_height(block_height, address)?;

        // Nothing to do if there are no UTXOs
        if spendable_utxos.is_empty() {
            return Ok(Vec::new());
        }

        // Sort by block height (FIFO) - earlier blocks first
        let mut sorted_utxos = spendable_utxos;
        sorted_utxos.sort_by_key(|utxo| utxo.block_height);

        let mut selected_utxos = Vec::new();
        let mut accumulated_amount = 0;

        for utxo in sorted_utxos {
            if accumulated_amount >= target_amount {
                break;
            }

            selected_utxos.push(utxo.clone());
            accumulated_amount += utxo.amount;
        }

        // Only return UTXOs if we have enough to meet the target amount
        if accumulated_amount >= target_amount {
            Ok(selected_utxos)
        } else {
            // We don't have enough funds
            Err(StorageError::InsufficientFunds {
                available: accumulated_amount,
                required: target_amount,
            })
        }
    }

    pub fn get_utxos_for_block_and_address(
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

        self.datasource
            .get_utxos_for_block_and_address(block_height, address)
    }

    pub fn get_block_txids(&self, block_height: i32) -> StorageResult<Vec<String>> {
        // Validate parameters
        if block_height < 0 {
            return Err(StorageError::InvalidBlockHeight(block_height));
        }

        let mut txids = HashSet::new();

        // Get all UTXOs for this block height from the datasource
        let utxos = self.datasource.get_all_utxos_for_block(block_height)?;

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

        Ok(txids.into_iter().collect())
    }
}
