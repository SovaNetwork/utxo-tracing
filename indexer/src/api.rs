use std::collections::HashSet;
use std::convert::Infallible;
use std::sync::Arc;

use bitcoincore_rpc::bitcoin::{address::NetworkUnchecked, Address, Network};
use log::{debug, error, info};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::RwLock;
use warp::{http::StatusCode, Filter, Reply};

#[derive(Clone)]
pub struct ApiState {
    pub watched_addresses: Arc<RwLock<HashSet<Address>>>,
    pub network: Network,
    pub enclave_url: String,
    pub utxo_url: String,
    pub enclave_api_key: String,
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
    pub inputs: Vec<serde_json::Value>,
    pub outputs: Vec<serde_json::Value>,
}

#[derive(Deserialize, Serialize)]
pub struct SignTransactionResponse {
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
    body: WatchAddressRequest,
    state: ApiState,
) -> Result<impl Reply, Infallible> {
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
    req: DeriveAddressRequest,
    state: ApiState,
) -> Result<impl Reply, Infallible> {
    let trimmed = req.evm_address.trim_start_matches("0x");

    let client = Client::new();
    let resp = client
        .post(format!("{}/derive_address", state.enclave_url))
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
    req: SelectUtxosRequest,
    state: ApiState,
) -> Result<impl Reply, Infallible> {
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

pub async fn sign_transaction_handler(
    req: SignTransactionRequest,
    state: ApiState,
) -> Result<impl Reply, Infallible> {
    info!(
        "Signing transaction with {} inputs and {} outputs",
        req.inputs.len(),
        req.outputs.len()
    );

    if state.enclave_api_key.trim().is_empty() {
        error!("ENCLAVE_API_KEY not configured");
        let resp = warp::reply::json(&json!({ "error": "ENCLAVE_API_KEY missing" }));
        return Ok(warp::reply::with_status(
            resp,
            StatusCode::INTERNAL_SERVER_ERROR,
        ));
    }

    debug!(
        "Forwarding signing request to enclave: {}",
        state.enclave_url
    );

    let client = Client::new();
    let resp = client
        .post(format!("{}/sign_transaction", state.enclave_url))
        .header("X-API-Key", state.enclave_api_key.clone())
        .json(&req)
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => match r.json::<SignTransactionResponse>().await {
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
                debug!("Enclave returned signed tx: {}", val.signed_tx);
                Ok(warp::reply::with_status(
                    warp::reply::json(&val),
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

pub async fn run_server(host: &str, port: u16, state: ApiState) {
    let post_route = warp::post()
        .and(warp::path("watch-address"))
        .and(warp::body::json())
        .and(with_state(state.clone()))
        .and_then(watch_address_handler);

    let get_route = warp::get()
        .and(warp::path("watch-address"))
        .and(warp::path::param())
        .and(with_state(state.clone()))
        .and_then(get_watch_address_handler);

    let derive_address_route = warp::post()
        .and(warp::path("derive-address"))
        .and(warp::body::json())
        .and(with_state(state.clone()))
        .and_then(derive_address_handler);

    let select_utxos_route = warp::post()
        .and(warp::path("select-utxos"))
        .and(warp::body::json())
        .and(with_state(state.clone()))
        .and_then(select_utxos_handler);

    let sign_tx_route = warp::post()
        .and(warp::path("sign-transaction"))
        .and(warp::body::json())
        .and(with_state(state))
        .and_then(sign_transaction_handler);

    let routes = post_route
        .or(get_route)
        .or(derive_address_route)
        .or(select_utxos_route)
        .or(sign_tx_route);

    let addr: std::net::IpAddr = host.parse().unwrap_or_else(|_| "0.0.0.0".parse().unwrap());
    warp::serve(routes).run((addr, port)).await;
}
