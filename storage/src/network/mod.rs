use std::sync::Arc;

use bitcoin::Network;

use crate::database::UtxoDatabase;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<UtxoDatabase>,
    /// The Bitcoin network this instance is operating on
    pub network: Network,
}

pub mod http;
pub mod socket;
