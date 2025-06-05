use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct SignedTx {
    pub id: i64,
    pub txid: String,
    pub signed_tx: String,
    pub caller: String,
    pub block_height: i32,
    pub amount: i64,
    pub destination: String,
    pub fee: i64,
    pub timestamp: DateTime<Utc>,
}
