use actix_web::{web, HttpResponse};
use serde_json::json;
use tracing::log::error;
use tracing::{debug, info, instrument};

use bitcoin::{Address, Network};
use std::str::FromStr;

use super::AppState;
use crate::error::StorageError;
use network_shared::StoreSignedTxRequest;
use serde::Deserialize;

fn parse_bitcoin_address(address: &str, expected_network: Network) -> Result<Address, String> {
    let addr =
        Address::from_str(address).map_err(|e| format!("Invalid Bitcoin address format: {e}"))?;

    addr.require_network(expected_network)
        .map_err(|e| format!("Bitcoin address network mismatch: {e}"))
}

pub fn configure_routes(cfg: &mut web::ServiceConfig) {
    cfg.route("/latest-block", web::get().to(get_latest_block))
        .route("/block/{height}/txids", web::get().to(get_block_txids))
        .route(
            "/utxos/block/{height}/address/{address}",
            web::get().to(get_block_address_utxos),
        )
        .route(
            "/spendable-utxos/block/{height}/address/{address}",
            web::get().to(get_spendable_utxos),
        )
        .route(
            "/select-utxos/block/{height}/address/{address}/amount/{amount}",
            web::get().to(select_utxos),
        )
        .route("/utxo/{txid}/{vout}", web::get().to(get_utxo_by_outpoint))
        .route("/signed-tx", web::post().to(store_signed_tx))
        .route("/signed-tx", web::get().to(get_all_signed_txs))
        .route("/signed-tx/{txid}", web::get().to(get_signed_tx))
        .route(
            "/signed-tx/caller/{caller}",
            web::get().to(get_signed_txs_by_caller),
        )
        .route(
            "/signed-tx/block/{height}",
            web::get().to(get_signed_txs_by_block),
        )
        .route(
            "/signed-tx/destination/{destination}",
            web::get().to(get_signed_txs_by_destination),
        );
}

#[instrument(skip(state))]
async fn get_latest_block(state: web::Data<AppState>) -> HttpResponse {
    info!("Getting latest block height");

    match state.db.get_latest_block() {
        Ok(latest_block) => {
            let response = json!({
                "latest_block": latest_block
            });
            HttpResponse::Ok().json(response)
        }
        Err(e) => {
            error!("Failed to get latest block: {e}");
            HttpResponse::InternalServerError().json(json!({
                "error": format!("Failed to retrieve latest block: {e}")
            }))
        }
    }
}

#[instrument(skip(state))]
async fn get_block_txids(state: web::Data<AppState>, path: web::Path<i32>) -> HttpResponse {
    let block_height = path.into_inner();

    info!(block_height, "Getting transaction IDs for block");

    // Validate block height
    if block_height < 0 {
        return HttpResponse::BadRequest().json(json!({
            "error": "Block height must be non-negative"
        }));
    }

    // Get latest block
    let latest_block = match state.db.get_latest_block() {
        Ok(block) => block,
        Err(e) => {
            error!("Failed to get latest block: {}", e);
            return HttpResponse::InternalServerError().json(json!({
                "error": format!("Failed to retrieve latest block: {}", e)
            }));
        }
    };

    if block_height > latest_block {
        return HttpResponse::NotFound().json(json!({
            "error": "Block not found",
            "latest_block": latest_block
        }));
    }

    // Get transactions
    match state.db.get_block_txids(block_height) {
        Ok(txids) => {
            let response = json!({
                "txids": txids
            });
            HttpResponse::Ok().json(response)
        }
        Err(e) => {
            error!("Failed to get txids for block {}: {}", block_height, e);
            match e {
                StorageError::BlockNotFound(_) => HttpResponse::NotFound().json(json!({
                    "error": format!("{}", e)
                })),
                _ => HttpResponse::InternalServerError().json(json!({
                    "error": format!("Failed to retrieve transaction IDs: {}", e)
                })),
            }
        }
    }
}

#[instrument(skip(state))]
async fn get_block_address_utxos(
    state: web::Data<AppState>,
    path: web::Path<(i32, String)>,
) -> HttpResponse {
    let (block_height, address) = path.into_inner();

    info!(block_height, %address, "Querying UTXOs for block and address");

    // Validate parameters
    if block_height < 0 {
        return HttpResponse::BadRequest().json(json!({
            "error": "Block height must be non-negative"
        }));
    }

    // Validate address format and network
    if let Err(msg) = parse_bitcoin_address(&address, state.network) {
        return HttpResponse::BadRequest().json(json!({ "error": msg }));
    }

    // Get latest block
    let latest_block = match state.db.get_latest_block() {
        Ok(block) => block,
        Err(e) => {
            error!("Failed to get latest block: {}", e);
            return HttpResponse::InternalServerError().json(json!({
                "error": format!("Failed to retrieve latest block: {}", e)
            }));
        }
    };

    if block_height > latest_block {
        return HttpResponse::NotFound().json(json!({
            "error": "Block not found",
            "latest_block": latest_block
        }));
    }

    // Get UTXOs
    match state
        .db
        .get_utxos_for_block_and_address(block_height, &address)
    {
        Ok(utxos) => {
            if utxos.is_empty() {
                return HttpResponse::NotFound().json(json!({
                    "error": "No UTXOs found for specified address in the specified block",
                }));
            }

            let response = json!({
                "block_height": block_height,
                "address": address,
                "utxos": utxos
            });
            HttpResponse::Ok().json(response)
        }
        Err(e) => {
            error!(
                "Failed to get UTXOs for block {} and address {}: {}",
                block_height, address, e
            );
            match e {
                StorageError::BlockNotFound(_) | StorageError::InvalidAddress(_) => {
                    HttpResponse::NotFound().json(json!({
                        "error": format!("{}", e)
                    }))
                }
                _ => HttpResponse::InternalServerError().json(json!({
                    "error": format!("Failed to retrieve UTXOs: {}", e)
                })),
            }
        }
    }
}

#[instrument(skip(state))]
async fn get_spendable_utxos(
    state: web::Data<AppState>,
    path: web::Path<(i32, String)>,
) -> HttpResponse {
    let (block_height, address) = path.into_inner();

    // Validate parameters
    if block_height < 0 {
        return HttpResponse::BadRequest().json(json!({
            "error": "Block height must be non-negative"
        }));
    }

    // Validate address format and network
    if let Err(msg) = parse_bitcoin_address(&address, state.network) {
        return HttpResponse::BadRequest().json(json!({ "error": msg }));
    }

    info!(block_height, %address, "Querying spendable UTXOs for address at height");

    // Check latest block
    let latest_block = match state.db.get_latest_block() {
        Ok(block) => block,
        Err(e) => {
            error!("Failed to get latest block: {}", e);
            return HttpResponse::InternalServerError().json(json!({
                "error": format!("Failed to retrieve latest block: {}", e)
            }));
        }
    };

    if block_height > latest_block {
        return HttpResponse::NotFound().json(json!({
            "error": "Block not found",
            "latest_block": latest_block
        }));
    }

    // Get UTXOs with proper error handling
    match state
        .db
        .get_spendable_utxos_at_height(block_height, &address)
    {
        Ok(spendable_utxos) => {
            let total_amount: i64 = spendable_utxos.iter().map(|utxo| utxo.amount).sum();

            let response = json!({
                "block_height": block_height,
                "address": address,
                "spendable_utxos": spendable_utxos,
                "total_amount": total_amount
            });

            HttpResponse::Ok().json(response)
        }
        Err(e) => {
            error!("Failed to get spendable UTXOs: {}", e);
            HttpResponse::InternalServerError().json(json!({
                "error": format!("Failed to retrieve UTXOs: {}", e)
            }))
        }
    }
}

#[instrument(skip(state))]
async fn select_utxos(
    state: web::Data<AppState>,
    path: web::Path<(i32, String, i64)>, // block_height, address, target_amount (satoshis)
) -> HttpResponse {
    let (block_height, address, target_amount) = path.into_inner();

    info!(
        block_height,
        %address,
        target_amount,
        "Selecting UTXOs for amount using FIFO"
    );

    // Validate parameters
    if block_height < 0 {
        return HttpResponse::BadRequest().json(json!({
            "error": "Block height must be non-negative"
        }));
    }

    // Validate address format and network
    if let Err(msg) = parse_bitcoin_address(&address, state.network) {
        return HttpResponse::BadRequest().json(json!({ "error": msg }));
    }

    if target_amount <= 0 {
        return HttpResponse::BadRequest().json(json!({
            "error": "Target amount must be greater than 0"
        }));
    }

    // Get latest block
    let latest_block = match state.db.get_latest_block() {
        Ok(block) => block,
        Err(e) => {
            error!("Failed to get latest block: {}", e);
            return HttpResponse::InternalServerError().json(json!({
                "error": format!("Failed to retrieve latest block: {}", e)
            }));
        }
    };

    if block_height > latest_block {
        return HttpResponse::NotFound().json(json!({
            "error": "Block not found",
            "latest_block": latest_block
        }));
    }

    // Select UTXOs
    match state
        .db
        .select_utxos_for_amount(block_height, &address, target_amount)
    {
        Ok(selected_utxos) => {
            let total_amount: i64 = selected_utxos.iter().map(|utxo| utxo.amount).sum();

            let response = json!({
                "block_height": block_height,
                "address": address,
                "target_amount": target_amount,
                "selected_utxos": selected_utxos,
                "total_amount": total_amount
            });

            HttpResponse::Ok().json(response)
        }
        Err(e) => match e {
            StorageError::InsufficientFunds {
                available,
                required,
            } => HttpResponse::NotFound().json(json!({
                "error": "Insufficient funds to meet target amount",
                "available_amount": available,
                "target_amount": required
            })),
            _ => {
                error!("Failed to select UTXOs: {}", e);
                HttpResponse::InternalServerError().json(json!({
                    "error": format!("Failed to select UTXOs: {}", e)
                }))
            }
        },
    }
}

#[instrument(skip(state))]
async fn get_utxo_by_outpoint(
    state: web::Data<AppState>,
    path: web::Path<(String, i32)>,
) -> HttpResponse {
    let (txid, vout) = path.into_inner();
    debug!(%txid, vout, "Fetching UTXO by outpoint");

    match state.db.get_utxo_by_outpoint(&txid, vout) {
        Ok(Some(utxo)) => HttpResponse::Ok().json(json!({ "utxo": utxo })),
        Ok(None) => HttpResponse::NotFound().json(json!({ "error": "UTXO not found" })),
        Err(e) => {
            error!("Failed to fetch UTXO: {}", e);
            HttpResponse::InternalServerError().json(json!({ "error": format!("{}", e) }))
        }
    }
}

#[instrument(skip(state, body))]
async fn store_signed_tx(
    state: web::Data<AppState>,
    body: web::Json<StoreSignedTxRequest>,
) -> HttpResponse {
    match state.signed_db.store_signed_tx(
        &body.txid,
        &body.signed_tx,
        &body.caller,
        body.block_height,
        body.amount,
        &body.destination,
        body.fee,
    ) {
        Ok(_) => HttpResponse::Ok().json(json!({ "status": "OK" })),
        Err(e) => {
            error!("Failed to store signed tx: {}", e);
            HttpResponse::InternalServerError().json(json!({ "error": format!("{}", e) }))
        }
    }
}

#[instrument(skip(state))]
async fn get_signed_tx(state: web::Data<AppState>, path: web::Path<String>) -> HttpResponse {
    let txid = path.into_inner();
    match state.signed_db.get_signed_tx_by_txid(&txid) {
        Ok(Some(tx)) => HttpResponse::Ok().json(json!({ "signed_tx": tx })),
        Ok(None) => {
            HttpResponse::NotFound().json(json!({ "error": "Signed transaction not found" }))
        }
        Err(e) => {
            error!("Failed to fetch signed tx: {}", e);
            HttpResponse::InternalServerError().json(json!({ "error": format!("{}", e) }))
        }
    }
}

#[instrument(skip(state))]
async fn get_signed_txs_by_caller(
    state: web::Data<AppState>,
    path: web::Path<String>,
) -> HttpResponse {
    let caller = path.into_inner();
    match state.signed_db.get_signed_txs_by_caller(&caller) {
        Ok(txs) => {
            if txs.is_empty() {
                HttpResponse::NotFound()
                    .json(json!({ "error": "No signed transactions found for caller" }))
            } else {
                HttpResponse::Ok().json(json!({ "caller": caller, "signed_txs": txs }))
            }
        }
        Err(e) => {
            error!("Failed to fetch signed txs: {}", e);
            HttpResponse::InternalServerError().json(json!({ "error": format!("{}", e) }))
        }
    }
}

#[instrument(skip(state))]
async fn get_signed_txs_by_block(state: web::Data<AppState>, path: web::Path<i32>) -> HttpResponse {
    let height = path.into_inner();
    match state.signed_db.get_signed_txs_by_block_height(height) {
        Ok(txs) => {
            if txs.is_empty() {
                HttpResponse::NotFound()
                    .json(json!({ "error": "No signed transactions found for block" }))
            } else {
                HttpResponse::Ok().json(json!({ "block_height": height, "signed_txs": txs }))
            }
        }
        Err(e) => {
            error!("Failed to fetch signed txs: {}", e);
            HttpResponse::InternalServerError().json(json!({ "error": format!("{}", e) }))
        }
    }
}

#[instrument(skip(state))]
async fn get_signed_txs_by_destination(
    state: web::Data<AppState>,
    path: web::Path<String>,
) -> HttpResponse {
    let dest = path.into_inner();
    match state.signed_db.get_signed_txs_by_destination(&dest) {
        Ok(txs) => {
            if txs.is_empty() {
                HttpResponse::NotFound()
                    .json(json!({ "error": "No signed transactions found for destination" }))
            } else {
                HttpResponse::Ok().json(json!({ "destination": dest, "signed_txs": txs }))
            }
        }
        Err(e) => {
            error!("Failed to fetch signed txs: {}", e);
            HttpResponse::InternalServerError().json(json!({ "error": format!("{}", e) }))
        }
    }
}

#[derive(Deserialize, Debug)]
struct PaginationQuery {
    page: Option<u32>,
}

#[instrument(skip(state))]
async fn get_all_signed_txs(
    state: web::Data<AppState>,
    query: web::Query<PaginationQuery>,
) -> HttpResponse {
    let page = query.page.unwrap_or(1);
    if page == 0 {
        return HttpResponse::BadRequest().json(json!({
            "error": "page must be greater than 0"
        }));
    }
    let limit = 250_i64;
    let offset = ((page - 1) as i64) * limit;
    match state.signed_db.get_signed_txs_paginated(offset, limit) {
        Ok(txs) => HttpResponse::Ok().json(json!({
            "page": page,
            "signed_txs": txs
        })),
        Err(e) => {
            error!("Failed to fetch signed txs: {}", e);
            HttpResponse::InternalServerError().json(json!({ "error": format!("{}", e) }))
        }
    }
}
