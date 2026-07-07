use near_sdk::json_types::Base64VecU8;
use near_sdk::{near, AccountId, Gas, NearToken, PublicKey};

const ACTION_FUNCTION_CALL: u8 = 2;
const ACTION_TRANSFER: u8 = 3;
const HEX: &[u8; 16] = b"0123456789abcdef";

#[near(serializers = [borsh, json])]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TxAction {
    FunctionCall {
        method_name: String,
        args: Base64VecU8,
        gas: Gas,
        deposit: NearToken,
    },
    Transfer {
        deposit: NearToken,
    },
}

pub fn mpc_path(account: &AccountId) -> String {
    format!("hos-tla/{account}")
}

pub fn build_transaction(
    signer: &AccountId,
    public_key: &PublicKey,
    nonce: u64,
    receiver: &AccountId,
    block_hash: &[u8; 32],
    actions: &[TxAction],
) -> Vec<u8> {
    let mut b = Vec::with_capacity(128 + actions.len() * 64);
    write_header(
        &mut b,
        signer,
        public_key,
        nonce,
        receiver,
        block_hash,
        actions.len(),
    );
    for action in actions {
        match action {
            TxAction::FunctionCall {
                method_name,
                args,
                gas,
                deposit,
            } => {
                b.push(ACTION_FUNCTION_CALL);
                write_str(&mut b, method_name);
                write_bytes(&mut b, &args.0);
                b.extend_from_slice(&gas.as_gas().to_le_bytes());
                b.extend_from_slice(&deposit.as_yoctonear().to_le_bytes());
            }
            TxAction::Transfer { deposit } => {
                b.push(ACTION_TRANSFER);
                b.extend_from_slice(&deposit.as_yoctonear().to_le_bytes());
            }
        }
    }
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

#[allow(clippy::too_many_arguments)]
fn write_header(
    b: &mut Vec<u8>,
    signer: &AccountId,
    public_key: &PublicKey,
    nonce: u64,
    receiver: &AccountId,
    block_hash: &[u8; 32],
    action_count: usize,
) {
    write_str(b, signer.as_str());
    b.extend_from_slice(public_key.as_bytes());
    b.extend_from_slice(&nonce.to_le_bytes());
    write_str(b, receiver.as_str());
    b.extend_from_slice(block_hash);
    b.extend_from_slice(&(action_count as u32).to_le_bytes());
}

pub fn write_str(b: &mut Vec<u8>, s: &str) {
    write_bytes(b, s.as_bytes());
}

pub fn write_bytes(b: &mut Vec<u8>, bytes: &[u8]) {
    b.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    b.extend_from_slice(bytes);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    const SIGNER: &str = "alice.beta.hostla.testnet";
    const SIGNING_KEY: &str = "ed25519:DZdWKDt29SBdPqeyfykg8TFF5Zkb5Qzdd6FJiJMvftZG";
    const NONCE: u64 = 42;

    const TRANSFER_CANONICAL: &str = "19000000616c6963652e626574612e686f73746c612e746573746e657400baa81b168f3b4d0c1af89664de30fa8d6754349847e0bba1020bf5ea43ff75fb2a000000000000000b000000626f622e746573746e65740000000000000000000000000000000000000000000000000000000000000000010000000315cd5b07000000000000000000000000";
    const FUNCTION_CALL_CANONICAL: &str = "19000000616c6963652e626574612e686f73746c612e746573746e657400baa81b168f3b4d0c1af89664de30fa8d6754349847e0bba1020bf5ea43ff75fb2a000000000000001700000072656769737472792e686f73746c612e746573746e657400000000000000000000000000000000000000000000000000000000000000000100000002100000006c6973745f7375625f6163636f756e74070000007b2278223a317d00c06e31d910010001000000000000000000000000000000";
    const MULTI_CANONICAL: &str = "19000000616c6963652e626574612e686f73746c612e746573746e657400baa81b168f3b4d0c1af89664de30fa8d6754349847e0bba1020bf5ea43ff75fb2a000000000000000b000000626f622e746573746e65740000000000000000000000000000000000000000000000000000000000000000020000000301000000000000000000000000000000020400000070696e6700000000005039278c04000000000000000000000000000000000000";

    fn signer() -> AccountId {
        AccountId::from_str(SIGNER).unwrap()
    }

    fn signing_key() -> PublicKey {
        PublicKey::from_str(SIGNING_KEY).unwrap()
    }

    fn acc(s: &str) -> AccountId {
        AccountId::from_str(s).unwrap()
    }

    #[test]
    fn transfer_matches_near_api_js_byte_for_byte() {
        let bytes = build_transaction(
            &signer(),
            &signing_key(),
            NONCE,
            &acc("bob.testnet"),
            &[0u8; 32],
            &[TxAction::Transfer {
                deposit: NearToken::from_yoctonear(123_456_789),
            }],
        );
        assert_eq!(to_hex(&bytes), TRANSFER_CANONICAL);
    }

    #[test]
    fn function_call_matches_near_api_js_byte_for_byte() {
        let bytes = build_transaction(
            &signer(),
            &signing_key(),
            NONCE,
            &acc("registry.hostla.testnet"),
            &[0u8; 32],
            &[TxAction::FunctionCall {
                method_name: "list_sub_account".to_string(),
                args: Base64VecU8(br#"{"x":1}"#.to_vec()),
                gas: Gas::from_tgas(300),
                deposit: NearToken::from_yoctonear(1),
            }],
        );
        assert_eq!(to_hex(&bytes), FUNCTION_CALL_CANONICAL);
    }

    #[test]
    fn multi_action_matches_near_api_js_byte_for_byte() {
        let bytes = build_transaction(
            &signer(),
            &signing_key(),
            NONCE,
            &acc("bob.testnet"),
            &[0u8; 32],
            &[
                TxAction::Transfer {
                    deposit: NearToken::from_yoctonear(1),
                },
                TxAction::FunctionCall {
                    method_name: "ping".to_string(),
                    args: Base64VecU8(Vec::new()),
                    gas: Gas::from_tgas(5),
                    deposit: NearToken::from_yoctonear(0),
                },
            ],
        );
        assert_eq!(to_hex(&bytes), MULTI_CANONICAL);
    }

    #[test]
    fn mpc_path_is_stable() {
        assert_eq!(
            mpc_path(&acc("alice.hos.near")),
            "hos-tla/alice.hos.near".to_string()
        );
    }
}
