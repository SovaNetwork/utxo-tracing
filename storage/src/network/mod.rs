use crate::database::UtxoDatabase;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<UtxoDatabase>,
}

pub mod http;
pub mod socket;
