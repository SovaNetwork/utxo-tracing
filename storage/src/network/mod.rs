use std::sync::Arc;

use bitcoin::Network;

use crate::database::UtxoDatabase;
use crate::signed_db::SignedTxDatabase;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<UtxoDatabase>,
    pub signed_db: Arc<SignedTxDatabase>,
    /// The Bitcoin network this instance is operating on
    pub network: Network,
}

pub mod http;
pub mod socket;
