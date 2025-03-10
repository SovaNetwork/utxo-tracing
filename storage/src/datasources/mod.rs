pub mod csv;
pub mod sqlite;

use std::sync::Arc;

use chrono::{DateTime, Utc};
use network_shared::UtxoUpdate;

use crate::error::{StorageError, StorageResult};
use crate::models::utxo::PendingChanges;
use crate::models::whitelist::WhitelistedAddress;

pub trait Datasource {
    fn setup(&self) -> StorageResult<()>;
    fn get_type(&self) -> String;
    fn get_latest_block(&self) -> StorageResult<i32>;
    fn process_block_utxos(&self, pending_changes: &PendingChanges) -> StorageResult<()>;
    fn get_spendable_utxos_at_height(
        &self,
        block_height: i32,
        address: &str,
    ) -> StorageResult<Vec<UtxoUpdate>>;
    fn get_utxos_for_block_and_address(
        &self,
        block_height: i32,
        address: &str,
    ) -> StorageResult<Vec<UtxoUpdate>>;
    fn get_all_utxos_for_block(&self, block_height: i32) -> StorageResult<Vec<UtxoUpdate>>;
    fn store_block(&self, height: i32, hash: &str, timestamp: DateTime<Utc>) -> StorageResult<()>;
    fn get_block_hash(&self, height: i32) -> StorageResult<Option<String>>;
    fn mark_blocks_as_final(&self, threshold: i32) -> StorageResult<()>;
    fn mark_blocks_after_height_not_main_chain(&self, height: i32) -> StorageResult<()>;
    fn revert_utxos_after_height(&self, height: i32) -> StorageResult<()>;
    fn add_whitelisted_address(&self, address: &str) -> StorageResult<()>;
    fn is_address_whitelisted(&self, address: &str) -> StorageResult<bool>;
    fn get_whitelisted_addresses(&self) -> StorageResult<Vec<WhitelistedAddress>>;
}

// Factory function to create datasource based on type
pub fn create_datasource(arg: &str) -> StorageResult<Arc<dyn Datasource + Send + Sync>> {
    match arg {
        "csv" => {
            let datasource = crate::datasources::csv::UtxoCSVDatasource::new();
            Ok(datasource as Arc<dyn Datasource + Send + Sync>)
        }
        "sqlite" => {
            let datasource = crate::datasources::sqlite::UtxoSqliteDatasource::new()?;
            Ok(datasource as Arc<dyn Datasource + Send + Sync>)
        }
        _ => Err(StorageError::UnexpectedError(format!(
            "Invalid argument for datasource: {}, Use 'csv' or 'sqlite'",
            arg
        ))),
    }
}
