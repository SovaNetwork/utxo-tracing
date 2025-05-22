use std::collections::HashSet;
use std::convert::Infallible;
use std::sync::Arc;

use bitcoincore_rpc::bitcoin::{address::NetworkUnchecked, Address, Network};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::RwLock;
use warp::{http::StatusCode, Filter, Reply};
use log::info;

#[derive(Clone)]
pub struct ApiState {
    pub watched_addresses: Arc<RwLock<HashSet<Address>>>,
    pub network: Network,
}

fn with_state(state: ApiState) -> impl Filter<Extract = (ApiState,), Error = Infallible> + Clone {
    warp::any().map(move || state.clone())
}

#[derive(Deserialize)]
pub struct WatchAddressRequest {
    pub btc_address: String,
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

pub async fn run_server(host: &str, port: u16, state: ApiState) {
    let post_route = warp::post()
        .and(warp::path("watch-address"))
        .and(warp::body::json())
        .and(with_state(state.clone()))
        .and_then(watch_address_handler);

    let get_route = warp::get()
        .and(warp::path("watch-address"))
        .and(warp::path::param())
        .and(with_state(state))
        .and_then(get_watch_address_handler);

    let routes = post_route.or(get_route);

    let addr: std::net::IpAddr = host.parse().unwrap_or_else(|_| "0.0.0.0".parse().unwrap());
    warp::serve(routes).run((addr, port)).await;
}

