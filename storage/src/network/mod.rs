use std::sync::Arc;

use crate::database::UtxoDatabase;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<UtxoDatabase>,
}

pub mod http;
pub mod socket;
