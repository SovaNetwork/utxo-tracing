use bitcoincore_rpc::bitcoin::{ScriptBuf, Witness};
use log::error;

/// Determines the type of a Bitcoin script
pub fn determine_script_type(script: ScriptBuf) -> String {
    if script.is_p2pkh() {
        "P2PKH".to_string()
    } else if script.is_p2sh() {
        "P2SH".to_string()
    } else if script.is_p2wpkh() {
        "P2WPKH".to_string()
    } else if script.is_p2wsh() {
        "P2WSH".to_string()
    } else if script.is_op_return() {
        "OP_RETURN".to_string()
    } else if script.is_witness_program() {
        "WITNESS".to_string()
    } else {
        error!("Unknown script type: {}", hex::encode(script.as_bytes()));
        "UNKNOWN".to_string()
    }
}

/// Extracts a public key from a Bitcoin witness
pub fn extract_public_key(witness: &Witness) -> Option<String> {
    if witness.is_empty() {
        return None;
    }
    witness.iter().nth(1).map(hex::encode)
}
