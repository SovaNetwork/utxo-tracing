use crate::error::Result;
use bitcoincore_rpc::bitcoin::{Address, Network, ScriptBuf, Witness};
use log::warn;

/// Determines the type of a Bitcoin script
pub fn determine_script_type(script: ScriptBuf) -> String {
    if script.is_p2pkh() {
        "P2PKH".to_string()
    } else if script.is_p2sh() {
        "P2SH".to_string()
    } else if script.is_v0_p2wpkh() {
        "P2WPKH".to_string()
    } else if script.is_v0_p2wsh() {
        "P2WSH".to_string()
    } else if script.is_op_return() {
        "OP_RETURN".to_string()
    } else if script.is_witness_program() {
        "WITNESS".to_string()
    } else if script.is_p2pk() {
        "P2PK".to_string()
    } else {
        warn!(
            "Nonstandard script type: {}",
            hex::encode(script.as_bytes())
        );
        "NONSTANDARD".to_string()
    }
}

/// Extracts an address from a Bitcoin script
pub fn extract_address(script: ScriptBuf, network: Network) -> Result<String> {
    match Address::from_script(&script, network) {
        Ok(addr) => Ok(addr.to_string()),
        Err(_) => {
            // For non-standard scripts, use a representation based on the script hash
            let script_hash = script.script_hash();
            Ok(format!("nonstandard:{}", script_hash))
        }
    }
}

/// Extracts a public key from a Bitcoin witness
pub fn extract_public_key(witness: &Witness) -> Option<String> {
    if witness.is_empty() {
        return None;
    }
    witness.iter().nth(1).map(|pk| hex::encode(pk))
}
