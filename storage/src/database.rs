use std::collections::HashSet;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use tracing::{info, instrument};

use network_shared::{BlockUpdate, UtxoUpdate, FINALITY_CONFIRMATIONS};

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

    pub async fn update_finality_status(&self, current_height: i32) -> StorageResult<()> {
        // Calculate new finality threshold
        let finality_threshold = current_height - FINALITY_CONFIRMATIONS + 1;

        if finality_threshold > 0 {
            // Mark all blocks at or below threshold as final
            self.datasource.mark_blocks_as_final(finality_threshold)?;

            info!(
                threshold = finality_threshold,
                "Updated finality status for blocks"
            );
        }

        Ok(())
    }

    pub async fn revert_to_height(&self, height: i32) -> StorageResult<()> {
        // If attempting to revert to a negative height, default to 0
        let height = std::cmp::max(height, 0);

        // Get current finality threshold (safely)
        let latest_height = match self.get_latest_block() {
            Ok(h) => h,
            Err(_) => 0, // Default to 0 if no blocks exist
        };

        // Never roll back past the finality threshold
        let finality_threshold = latest_height - network_shared::FINALITY_CONFIRMATIONS + 1;
        let finality_threshold = std::cmp::max(finality_threshold, 0); // Prevent negative thresholds

        // Safe revert height
        let safe_revert_height = std::cmp::max(height, finality_threshold - 1);

        // Log the revert operation
        if safe_revert_height != height {
            info!(
                requested_height = height,
                actual_height = safe_revert_height,
                "Limiting rollback to respect finality assumption"
            );
        }

        // If there are no blocks yet, there's nothing to revert
        if latest_height == 0 {
            return Ok(());
        }

        // Revert non-final blocks
        self.datasource
            .mark_blocks_after_height_not_main_chain(safe_revert_height)?;
        self.datasource
            .revert_utxos_after_height(safe_revert_height)?;

        Ok(())
    }

    pub fn get_block_hash(&self, height: i32) -> StorageResult<String> {
        match self.datasource.get_block_hash(height)? {
            Some(hash) => Ok(hash),
            None => Err(StorageError::BlockNotFound(height)),
        }
    }

    pub fn store_block(
        &self,
        height: i32,
        hash: &str,
        timestamp: DateTime<Utc>,
    ) -> StorageResult<()> {
        self.datasource.store_block(height, hash, timestamp)
    }
}
