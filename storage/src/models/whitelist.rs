use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct WhitelistedAddress {
    pub address: String,
    pub added_at: DateTime<Utc>,
}
