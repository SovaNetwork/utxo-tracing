use chrono::{DateTime, Utc};
use network_shared::UtxoUpdate;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct UtxoRow {
    // Key fields for storage
    pub address: String,
    pub utxo_id: String,

    // UTXO data fields
    pub id: String,
    pub utxo_address: String,
    pub public_key: Option<String>,
    pub txid: String,
    pub vout: i32,
    pub amount: i64,
    pub script_pub_key: String,
    pub script_type: String,
    pub created_at: DateTime<Utc>,
    pub block_height: i32,
    pub spent_txid: Option<String>,
    pub spent_at: Option<DateTime<Utc>>,
    pub spent_block: Option<i32>,
}

impl UtxoRow {
    pub fn into_storage_entry(self) -> (String, String, UtxoUpdate) {
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
pub struct BlockRow {
    // Key fields
    pub height: i32,
    pub address: String,

    // UTXO data fields
    pub id: String,
    pub utxo_address: String,
    pub public_key: Option<String>,
    pub txid: String,
    pub vout: i32,
    pub amount: i64,
    pub script_pub_key: String,
    pub script_type: String,
    pub created_at: DateTime<Utc>,
    pub block_height: i32,
    pub spent_txid: Option<String>,
    pub spent_at: Option<DateTime<Utc>>,
    pub spent_block: Option<i32>,
}

impl BlockRow {
    pub fn into_utxo(self) -> UtxoUpdate {
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

pub struct PendingChanges {
    pub height: i32,
    pub utxos_update: Vec<UtxoUpdate>, // modified UTXOs
    pub utxos_insert: Vec<UtxoUpdate>, // new UTXOs
}
