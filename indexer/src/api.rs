use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bitcoincore_rpc::bitcoin::{address::NetworkUnchecked, Address, Amount, Network};
use log::{debug, error, info};
use network_shared::StoreSignedTxRequest;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::str::FromStr;
use tokio::sync::RwLock;
use warp::{http::StatusCode, Filter, Reply};

/// Maximum allowed size for JSON request bodies (32 KiB)
const MAX_JSON_BODY_SIZE: u64 = 32 * 1024;

#[derive(Clone)]
pub struct ApiState {
    pub watched_addresses: Arc<RwLock<HashSet<Address>>>,
    pub network: Network,
    pub enclave_url: String,
    pub utxo_url: String,
    pub enclave_api_key: String,
    pub indexer_api_key: String,
    pub prepare_tx_cache: Arc<RwLock<HashMap<String, CachedPrepareResponse>>>,
}

fn with_state(state: ApiState) -> impl Filter<Extract = (ApiState,), Error = Infallible> + Clone {
    warp::any().map(move || state.clone())
}

#[derive(Deserialize)]
pub struct WatchAddressRequest {
    pub btc_address: String,
}

#[derive(Deserialize)]
pub struct DeriveAddressRequest {
    pub evm_address: String,
}

#[derive(Deserialize)]
pub struct DeriveAddressResponse {
    pub address: String,
}

#[derive(Deserialize)]
pub struct SelectUtxosRequest {
    pub block_height: i32,
    pub target_amount: i64,
}

#[derive(Deserialize, Serialize)]
pub struct SignTransactionRequest {
    /// block height to get spendable txs for
    pub block_height: i32,
    /// nominal amount to send
    pub amount: i64,
    /// receiving pubkey on btc
    pub destination: String,
    /// btc tx fee
    pub fee: i64,
    /// smart contract caller EVM address
    pub caller: String,
}

#[derive(Deserialize, Serialize)]
pub struct SignTransactionResponse {
    pub signed_tx: String,
    pub txid: String,
}

#[derive(Deserialize, Serialize)]
pub struct PrepareTransactionRequest {
    pub block_height: i32,
    pub amount: i64,
    pub destination: String,
    pub fee: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct PrepareTransactionResponse {
    pub txid: String,
}

#[derive(Debug, Clone)]
pub struct CachedPrepareResponse {
    pub response: PrepareTransactionResponse,
    pub created_at: Instant,
}

const CACHE_EXPIRY_DURATION: Duration = Duration::from_secs(300);

#[derive(Deserialize)]
struct EnclaveSignResponse {
    pub signed_tx: String,
}

fn parse_bitcoin_address(addr: &str, network: Network) -> Result<Address, String> {
    let address_unchecked: Address<NetworkUnchecked> = addr
        .parse()
        .map_err(|e| format!("Address parse error: {:?}", e))?;

    address_unchecked
        .require_network(network)
        .map_err(|e| format!("Network mismatch: {:?}", e))
}

pub async fn watch_address_handler(
    api_key: String,
    body: WatchAddressRequest,
    state: ApiState,
) -> Result<impl Reply, Infallible> {
    if api_key != state.indexer_api_key {
        let resp = warp::reply::json(&json!({ "error": "Unauthorized" }));
        return Ok(warp::reply::with_status(resp, StatusCode::UNAUTHORIZED));
    }
    match parse_bitcoin_address(&body.btc_address, state.network) {
        Ok(addr) => {
            {
                let mut set = state.watched_addresses.write().await;
                set.insert(addr.clone());
            }
            info!("Added watched address {}", addr);
            let resp = warp::reply::json(&json!({ "status": "OK" }));
            Ok(warp::reply::with_status(resp, StatusCode::OK))
        }
        Err(_) => {
            let resp = warp::reply::json(&json!({ "error": "Invalid BTC address" }));
            Ok(warp::reply::with_status(resp, StatusCode::BAD_REQUEST))
        }
    }
}

pub async fn get_watch_address_handler(
    btc_address: String,
    state: ApiState,
) -> Result<impl Reply, Infallible> {
    match parse_bitcoin_address(&btc_address, state.network) {
        Ok(addr) => {
            let set = state.watched_addresses.read().await;
            let watched = set.contains(&addr);
            let resp = warp::reply::json(&json!({ "watched": watched }));
            Ok(warp::reply::with_status(resp, StatusCode::OK))
        }
        Err(_) => {
            let resp = warp::reply::json(&json!({ "error": "Invalid BTC address" }));
            Ok(warp::reply::with_status(resp, StatusCode::BAD_REQUEST))
        }
    }
}

pub async fn derive_address_handler(
    api_key: String,
    req: DeriveAddressRequest,
    state: ApiState,
) -> Result<impl Reply, Infallible> {
    if api_key != state.indexer_api_key {
        let resp = warp::reply::json(&json!({ "error": "Unauthorized" }));
        return Ok(warp::reply::with_status(resp, StatusCode::UNAUTHORIZED));
    }
    let trimmed = req.evm_address.trim_start_matches("0x");

    if state.enclave_api_key.trim().is_empty() {
        error!("ENCLAVE_API_KEY not configured");
        let resp = warp::reply::json(&json!({ "error": "ENCLAVE_API_KEY missing" }));
        return Ok(warp::reply::with_status(
            resp,
            StatusCode::INTERNAL_SERVER_ERROR,
        ));
    }

    let client = Client::new();
    let resp = client
        .post(format!("{}/derive_address", state.enclave_url))
        .header("X-API-Key", state.enclave_api_key.clone())
        .json(&json!({ "evm_address": trimmed }))
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => match r.json::<DeriveAddressResponse>().await {
            Ok(derive_resp) => {
                if let Ok(addr) = parse_bitcoin_address(&derive_resp.address, state.network) {
                    let mut set = state.watched_addresses.write().await;
                    set.insert(addr);
                }
                let reply = warp::reply::json(&json!({ "btc_address": derive_resp.address }));
                Ok(warp::reply::with_status(reply, StatusCode::OK))
            }
            Err(_) => {
                let resp = warp::reply::json(&json!({ "error": "Invalid response" }));
                Ok(warp::reply::with_status(
                    resp,
                    StatusCode::INTERNAL_SERVER_ERROR,
                ))
            }
        },
        _ => {
            let resp = warp::reply::json(&json!({ "error": "Failed to derive" }));
            Ok(warp::reply::with_status(
                resp,
                StatusCode::INTERNAL_SERVER_ERROR,
            ))
        }
    }
}

pub async fn select_utxos_handler(
    api_key: String,
    req: SelectUtxosRequest,
    state: ApiState,
) -> Result<impl Reply, Infallible> {
    if api_key != state.indexer_api_key {
        let resp = warp::reply::json(&json!({ "error": "Unauthorized" }));
        return Ok(warp::reply::with_status(resp, StatusCode::UNAUTHORIZED));
    }
    let client = Client::new();

    let addresses: Vec<Address> = {
        let set = state.watched_addresses.read().await;
        set.iter().cloned().collect()
    };

    let mut utxos: Vec<network_shared::UtxoUpdate> = Vec::new();

    for addr in addresses {
        let url = format!(
            "{}/spendable-utxos/block/{}/address/{}",
            state.utxo_url, req.block_height, addr
        );

        if let Ok(resp) = client.get(&url).send().await {
            if resp.status().is_success() {
                if let Ok(value) = resp.json::<serde_json::Value>().await {
                    if let Some(list) = value.get("spendable_utxos") {
                        if let Ok(list) =
                            serde_json::from_value::<Vec<network_shared::UtxoUpdate>>(list.clone())
                        {
                            utxos.extend(list);
                        }
                    }
                }
            }
        }
    }

    utxos.sort_by_key(|u| u.block_height);

    let mut selected = Vec::new();
    let mut total = 0i64;

    for utxo in utxos.into_iter() {
        if total >= req.target_amount {
            break;
        }
        selected.push(utxo.clone());
        total += utxo.amount;
    }

    if total < req.target_amount {
        let resp = warp::reply::json(&json!({ "error": "Insufficient funds" }));
        return Ok(warp::reply::with_status(resp, StatusCode::BAD_REQUEST));
    }

    let resp = warp::reply::json(&json!({
        "selected_utxos": selected,
        "total": total
    }));
    Ok(warp::reply::with_status(resp, StatusCode::OK))
}

async fn fetch_selected_utxos(
    state: &ApiState,
    block_height: i32,
    total_needed: i64,
) -> Result<Vec<network_shared::UtxoUpdate>, (serde_json::Value, StatusCode)> {
    let client = reqwest::Client::new();
    let mut selected_utxos = Vec::new();
    let mut total_selected = 0;

    // Acquire read lock on watched addresses
    let watched_addresses = state.watched_addresses.read().await;

    // Loop over each address and attempt to gather enough UTXOs
    for addr in watched_addresses.iter() {
        let url = format!(
            "{}/select-utxos/block/{}/address/{}/amount/{}",
            state.utxo_url,
            block_height,
            addr,
            total_needed - total_selected
        );

        let resp = client.get(&url).send().await.map_err(|e| {
            error!("Failed to request UTXOs: {}", e);
            (
                json!({ "error": "Failed to fetch UTXOs" }),
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err_text = resp
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            error!("UTXO service error (status {}): {}", status, err_text);
            return Err((
                json!({ "error": format!("UTXO service error: {}", err_text) }),
                StatusCode::BAD_GATEWAY,
            ));
        }

        let value = resp.json::<serde_json::Value>().await.map_err(|e| {
            error!("Failed to parse UTXO response: {}", e);
            (
                json!({ "error": "Invalid UTXO response" }),
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })?;

        let utxos: Vec<network_shared::UtxoUpdate> =
            serde_json::from_value(value.get("selected_utxos").cloned().unwrap_or_default())
                .map_err(|e| {
                    error!("UTXO JSON structure incorrect: {}", e);
                    (
                        json!({ "error": "Malformed UTXO data" }),
                        StatusCode::INTERNAL_SERVER_ERROR,
                    )
                })?;

        for utxo in utxos {
            total_selected += utxo.amount;
            selected_utxos.push(utxo);

            if total_selected >= total_needed {
                debug!("Collected sufficient UTXOs: {}", total_selected);
                return Ok(selected_utxos);
            }
        }
    }

    // If we reach here, we couldn't gather enough UTXOs
    error!(
        "Insufficient UTXOs: Needed {}, got {}",
        total_needed, total_selected
    );
    Err((
        json!({ "error": "Insufficient funds across watched addresses" }),
        StatusCode::BAD_REQUEST,
    ))
}

fn generate_prepare_tx_cache_key(req: &PrepareTransactionRequest) -> String {
    let mut hasher = Sha256::new();
    hasher.update(req.block_height.to_le_bytes());
    hasher.update(req.amount.to_le_bytes());
    hasher.update(req.fee.to_le_bytes());
    hasher.update(req.destination.as_bytes());
    format!("{:x}", hasher.finalize())
}

async fn clean_expired_cache_entries(cache: &Arc<RwLock<HashMap<String, CachedPrepareResponse>>>) {
    let mut cache_write = cache.write().await;
    let now = Instant::now();
    cache_write.retain(|_, entry| now.duration_since(entry.created_at) < CACHE_EXPIRY_DURATION);
}

async fn fetch_selected_utxos_deterministic(
    state: &ApiState,
    block_height: i32,
    total_needed: i64,
) -> Result<Vec<network_shared::UtxoUpdate>, (serde_json::Value, StatusCode)> {
    let client = reqwest::Client::new();
    let mut selected_utxos = Vec::new();
    let mut total_selected = 0;

    let watched_addresses = {
        let addr_set = state.watched_addresses.read().await;
        let mut addresses: Vec<_> = addr_set.iter().cloned().collect();
        addresses.sort_by_key(|a| a.to_string());
        addresses
    };

    for addr in watched_addresses.iter() {
        if total_selected >= total_needed {
            break;
        }

        let remaining_needed = total_needed - total_selected;
        let url = format!(
            "{}/select-utxos/block/{}/address/{}/amount/{}",
            state.utxo_url, block_height, addr, remaining_needed
        );

        let resp = client.get(&url).send().await.map_err(|e| {
            error!("Failed to request UTXOs: {}", e);
            (
                json!({ "error": "Failed to fetch UTXOs" }),
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err_text = resp
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            error!("UTXO service error (status {}): {}", status, err_text);
            return Err((
                json!({ "error": format!("UTXO service error: {}", err_text) }),
                StatusCode::BAD_GATEWAY,
            ));
        }

        let value = resp.json::<serde_json::Value>().await.map_err(|e| {
            error!("Failed to parse UTXO response: {}", e);
            (
                json!({ "error": "Invalid UTXO response" }),
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })?;

        let utxos: Vec<network_shared::UtxoUpdate> =
            serde_json::from_value(value.get("selected_utxos").cloned().unwrap_or_default())
                .map_err(|e| {
                    error!("UTXO JSON structure incorrect: {}", e);
                    (
                        json!({ "error": "Malformed UTXO data" }),
                        StatusCode::INTERNAL_SERVER_ERROR,
                    )
                })?;

        for utxo in utxos {
            total_selected += utxo.amount;
            selected_utxos.push(utxo);

            if total_selected >= total_needed {
                debug!("Collected sufficient UTXOs: {}", total_selected);
                break;
            }
        }
    }

    if total_selected < total_needed {
        error!(
            "Insufficient UTXOs: Needed {}, got {}",
            total_needed, total_selected
        );
        return Err((
            json!({ "error": "Insufficient funds across watched addresses" }),
            StatusCode::BAD_REQUEST,
        ));
    }

    Ok(selected_utxos)
}

async fn store_signed_tx(
    state: &ApiState,
    req: &StoreSignedTxRequest,
) -> Result<(), reqwest::Error> {
    let client = reqwest::Client::new();
    let url = format!("{}/signed-tx", state.utxo_url);
    let body = json!(req);
    let resp = client.post(&url).json(&body).send().await?;
    if resp.status().is_success() {
        Ok(())
    } else {
        error!("Store signed tx failed with status {}", resp.status());
        Ok(())
    }
}

fn build_unsigned_transaction(
    selected_utxos: &[network_shared::UtxoUpdate],
    amount: i64,
    fee: i64,
    destination: &str,
    network: Network,
) -> Result<(bitcoincore_rpc::bitcoin::Transaction, i64), (serde_json::Value, StatusCode)> {
    use bitcoincore_rpc::bitcoin::{self, OutPoint, Sequence, Transaction, TxIn, TxOut, Witness};

    let total_needed = amount + fee;
    let total_selected: i64 = selected_utxos.iter().map(|u| u.amount).sum();
    if total_selected < total_needed {
        return Err((
            json!({ "error": "Insufficient funds" }),
            StatusCode::BAD_REQUEST,
        ));
    }

    let dest_addr = parse_bitcoin_address(destination, network).map_err(|_| {
        (
            json!({ "error": "Invalid destination" }),
            StatusCode::BAD_REQUEST,
        )
    })?;

    let mut inputs = Vec::new();
    for utxo in selected_utxos {
        if let Ok(txid) = bitcoin::Txid::from_str(&utxo.txid) {
            let outpoint = OutPoint {
                txid,
                vout: utxo.vout as u32,
            };
            inputs.push(TxIn {
                previous_output: outpoint,
                script_sig: bitcoin::ScriptBuf::new(),
                sequence: Sequence::MAX,
                witness: Witness::new(),
            });
        }
    }

    let mut outputs = Vec::new();
    outputs.push(TxOut {
        value: Amount::from_sat(amount as u64),
        script_pubkey: dest_addr.script_pubkey(),
    });

    let change = total_selected - total_needed;
    if change > 0 {
        if let Some(first) = selected_utxos.first() {
            if let Ok(change_addr) = parse_bitcoin_address(&first.address, network) {
                outputs.push(TxOut {
                    value: Amount::from_sat(change as u64),
                    script_pubkey: change_addr.script_pubkey(),
                });
            }
        }
    }

    let unsigned_tx = Transaction {
        version: bitcoin::transaction::Version::TWO,
        lock_time: bitcoin::locktime::absolute::LockTime::ZERO,
        input: inputs,
        output: outputs,
    };

    Ok((unsigned_tx, change))
}

pub async fn sign_transaction_handler(
    api_key: String,
    req: SignTransactionRequest,
    state: ApiState,
) -> Result<impl Reply, Infallible> {
    if api_key != state.indexer_api_key {
        let resp = warp::reply::json(&json!({ "error": "Unauthorized" }));
        return Ok(warp::reply::with_status(resp, StatusCode::UNAUTHORIZED));
    }

    if state.enclave_api_key.trim().is_empty() {
        error!("ENCLAVE_API_KEY not configured");
        let resp = warp::reply::json(&json!({ "error": "ENCLAVE_API_KEY missing" }));
        return Ok(warp::reply::with_status(
            resp,
            StatusCode::INTERNAL_SERVER_ERROR,
        ));
    }

    let total_needed = req.amount + req.fee;
    let selected_utxos: Vec<network_shared::UtxoUpdate> =
        match fetch_selected_utxos(&state, req.block_height, total_needed).await {
            Ok(list) => list,
            Err((msg, status)) => {
                return Ok(warp::reply::with_status(warp::reply::json(&msg), status));
            }
        };

    info!(
        "Building transaction to {} for {} sats using {} UTXOs",
        req.destination,
        req.amount,
        selected_utxos.len()
    );

    let (unsigned_tx, change) = match build_unsigned_transaction(
        &selected_utxos,
        req.amount,
        req.fee,
        &req.destination,
        state.network,
    ) {
        Ok(res) => res,
        Err((msg, status)) => {
            return Ok(warp::reply::with_status(warp::reply::json(&msg), status));
        }
    };

    let txid = unsigned_tx.txid().to_string();

    let enc_inputs: Vec<_> = selected_utxos
        .iter()
        .map(|u| {
            json!({
                "txid": u.txid,
                "vout": u.vout,
                "amount": u.amount,
                "address": u.address
            })
        })
        .collect();
    let mut enc_outputs = vec![json!({
        "address": req.destination,
        "amount": req.amount
    })];
    if change > 0 {
        if let Some(first) = selected_utxos.first() {
            enc_outputs.push(json!({
                "address": first.address,
                "amount": change
            }));
        }
    }

    let client = Client::new();
    let sign_resp = client
        .post(format!("{}/sign_transaction", state.enclave_url))
        .header("X-API-Key", state.enclave_api_key.clone())
        .json(&json!({ "inputs": enc_inputs, "outputs": enc_outputs }))
        .send()
        .await;

    match sign_resp {
        Ok(r) if r.status().is_success() => match r.json::<EnclaveSignResponse>().await {
            Ok(val) => {
                if hex::decode(&val.signed_tx).is_err() {
                    error!("Enclave returned invalid hex");
                    let resp = warp::reply::json(&json!({ "error": "Invalid response" }));
                    return Ok(warp::reply::with_status(
                        resp,
                        StatusCode::INTERNAL_SERVER_ERROR,
                    ));
                }
                info!("Successfully signed transaction");
                let reply = SignTransactionResponse {
                    signed_tx: val.signed_tx.clone(),
                    txid: txid.clone(),
                };

                // Fire and forget storing of the signed transaction
                let store_state = state.clone();
                let store_req = StoreSignedTxRequest {
                    txid: txid.clone(),
                    signed_tx: val.signed_tx.clone(),
                    caller: req.caller.clone(),
                    block_height: req.block_height,
                    amount: req.amount,
                    destination: req.destination.clone(),
                    fee: req.fee,
                };
                tokio::spawn(async move {
                    if let Err(e) = store_signed_tx(&store_state, &store_req).await {
                        error!("Failed to store signed tx: {}", e);
                    }
                });

                Ok(warp::reply::with_status(
                    warp::reply::json(&reply),
                    StatusCode::OK,
                ))
            }
            Err(e) => {
                error!("Failed to parse enclave response: {:?}", e);
                let resp = warp::reply::json(&json!({ "error": "Invalid response" }));
                Ok(warp::reply::with_status(
                    resp,
                    StatusCode::INTERNAL_SERVER_ERROR,
                ))
            }
        },
        Ok(r) => {
            let status = r.status();
            let err = r.text().await.unwrap_or_else(|_| "Failed".to_string());
            error!("Enclave signing failed: {}", err);
            Ok(warp::reply::with_status(
                warp::reply::json(&json!({ "error": err })),
                status,
            ))
        }
        Err(e) => {
            error!("Failed to contact enclave: {}", e);
            let resp = warp::reply::json(&json!({ "error": "Failed to contact enclave" }));
            Ok(warp::reply::with_status(
                resp,
                StatusCode::INTERNAL_SERVER_ERROR,
            ))
        }
    }
}

pub async fn prepare_transaction_handler_idempotent(
    api_key: String,
    req: PrepareTransactionRequest,
    state: ApiState,
) -> Result<impl Reply, Infallible> {
    if api_key != state.indexer_api_key {
        let resp = warp::reply::json(&json!({ "error": "Unauthorized" }));
        return Ok(warp::reply::with_status(resp, StatusCode::UNAUTHORIZED));
    }

    let cache_key = generate_prepare_tx_cache_key(&req);
    clean_expired_cache_entries(&state.prepare_tx_cache).await;

    {
        let cache_read = state.prepare_tx_cache.read().await;
        if let Some(cached_entry) = cache_read.get(&cache_key) {
            if Instant::now().duration_since(cached_entry.created_at) < CACHE_EXPIRY_DURATION {
                info!(
                    "Returning cached prepare transaction result for key: {}",
                    cache_key
                );
                return Ok(warp::reply::with_status(
                    warp::reply::json(&cached_entry.response),
                    StatusCode::OK,
                ));
            }
        }
    }

    let total_needed = req.amount + req.fee;

    let selected_utxos =
        match fetch_selected_utxos_deterministic(&state, req.block_height, total_needed).await {
            Ok(list) => list,
            Err((msg, status)) => {
                return Ok(warp::reply::with_status(warp::reply::json(&msg), status));
            }
        };

    info!(
        "Building transaction to {} for {} sats using {} UTXOs",
        req.destination,
        req.amount,
        selected_utxos.len()
    );

    let (unsigned_tx, _change) = match build_unsigned_transaction(
        &selected_utxos,
        req.amount,
        req.fee,
        &req.destination,
        state.network,
    ) {
        Ok(res) => res,
        Err((msg, status)) => {
            return Ok(warp::reply::with_status(warp::reply::json(&msg), status));
        }
    };

    let txid = unsigned_tx.txid().to_string();
    let response = PrepareTransactionResponse { txid };

    {
        let mut cache_write = state.prepare_tx_cache.write().await;
        cache_write.insert(
            cache_key.clone(),
            CachedPrepareResponse {
                response: response.clone(),
                created_at: Instant::now(),
            },
        );
    }

    info!("Cached prepare transaction result for key: {}", cache_key);

    Ok(warp::reply::with_status(
        warp::reply::json(&response),
        StatusCode::OK,
    ))
}

pub async fn run_server(host: &str, port: u16, state: ApiState) {
    let json_watch_address = warp::body::content_length_limit(MAX_JSON_BODY_SIZE)
        .and(warp::body::json::<WatchAddressRequest>());

    let json_derive_address = warp::body::content_length_limit(MAX_JSON_BODY_SIZE)
        .and(warp::body::json::<DeriveAddressRequest>());

    let json_select_utxos = warp::body::content_length_limit(MAX_JSON_BODY_SIZE)
        .and(warp::body::json::<SelectUtxosRequest>());

    let json_prepare_tx = warp::body::content_length_limit(MAX_JSON_BODY_SIZE)
        .and(warp::body::json::<PrepareTransactionRequest>());

    let json_sign_tx = warp::body::content_length_limit(MAX_JSON_BODY_SIZE)
        .and(warp::body::json::<SignTransactionRequest>());

    let post_route = warp::post()
        .and(warp::path("watch-address"))
        .and(warp::header::<String>("x-api-key"))
        .and(json_watch_address)
        .and(with_state(state.clone()))
        .and_then(watch_address_handler);

    let get_route = warp::get()
        .and(warp::path("watch-address"))
        .and(warp::path::param())
        .and(with_state(state.clone()))
        .and_then(get_watch_address_handler);

    let derive_address_route = warp::post()
        .and(warp::path("derive-address"))
        .and(warp::header::<String>("x-api-key"))
        .and(json_derive_address)
        .and(with_state(state.clone()))
        .and_then(derive_address_handler);

    let select_utxos_route = warp::post()
        .and(warp::path("select-utxos"))
        .and(warp::header::<String>("x-api-key"))
        .and(json_select_utxos)
        .and(with_state(state.clone()))
        .and_then(select_utxos_handler);

    let prepare_tx_route = warp::post()
        .and(warp::path("prepare-transaction"))
        .and(warp::header::<String>("x-api-key"))
        .and(json_prepare_tx)
        .and(with_state(state.clone()))
        .and_then(prepare_transaction_handler_idempotent);

    let sign_tx_route = warp::post()
        .and(warp::path("sign-transaction"))
        .and(warp::header::<String>("x-api-key"))
        .and(json_sign_tx)
        .and(with_state(state))
        .and_then(sign_transaction_handler);

    let routes = post_route
        .or(get_route)
        .or(derive_address_route)
        .or(select_utxos_route)
        .or(prepare_tx_route)
        .or(sign_tx_route);

    let addr: std::net::IpAddr = host.parse().unwrap_or_else(|_| "0.0.0.0".parse().unwrap());
    warp::serve(routes).run((addr, port)).await;
}
