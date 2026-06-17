use near_sdk::{AccountId, PublicKey};

const ACTION_ADD_KEY: u8 = 5;
const PERMISSION_FULL_ACCESS: u8 = 1;
const HEX: &[u8; 16] = b"0123456789abcdef";

pub fn add_key_tx(
    account: &AccountId,
    signing_key: &PublicKey,
    nonce: u64,
    block_hash: &[u8; 32],
    new_key: &PublicKey,
) -> Vec<u8> {
    let mut b = Vec::with_capacity(160);
    write_str(&mut b, account.as_str());
    b.extend_from_slice(signing_key.as_bytes());
    b.extend_from_slice(&nonce.to_le_bytes());
    write_str(&mut b, account.as_str());
    b.extend_from_slice(block_hash);
    b.extend_from_slice(&1u32.to_le_bytes());
    b.push(ACTION_ADD_KEY);
    b.extend_from_slice(new_key.as_bytes());
    b.extend_from_slice(&0u64.to_le_bytes());
    b.push(PERMISSION_FULL_ACCESS);
    b
}

pub fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        s.push(HEX[(byte >> 4) as usize] as char);
        s.push(HEX[(byte & 0x0f) as usize] as char);
    }
    s
}

fn write_str(b: &mut Vec<u8>, s: &str) {
    b.extend_from_slice(&(s.len() as u32).to_le_bytes());
    b.extend_from_slice(s.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    const NEAR_API_JS_CANONICAL: &str = "0e00000076696374696d2e746573746e657400baa81b168f3b4d0c1af89664de30fa8d6754349847e0bba1020bf5ea43ff75fb01000000000000000e00000076696374696d2e746573746e65740000000000000000000000000000000000000000000000000000000000000000010000000500f423be8411811b08763736d3b1b6fdd73ef6c4d68ba3016a834d5bfa4a671108000000000000000001";

    #[test]
    fn add_key_tx_matches_near_api_js_byte_for_byte() {
        let account = AccountId::from_str("victim.testnet").unwrap();
        let signing_key =
            PublicKey::from_str("ed25519:DZdWKDt29SBdPqeyfykg8TFF5Zkb5Qzdd6FJiJMvftZG").unwrap();
        let new_key =
            PublicKey::from_str("ed25519:HS26FofzajFx79BePttxKMFEFtJDvgpTdjajSJaif4jy").unwrap();
        let bytes = add_key_tx(&account, &signing_key, 1, &[0u8; 32], &new_key);
        assert_eq!(to_hex(&bytes), NEAR_API_JS_CANONICAL);
    }

    #[test]
    fn to_hex_encodes_low_and_high_nibbles() {
        assert_eq!(to_hex(&[0x00, 0x0f, 0xff, 0xa7]), "000fffa7");
    }
}
