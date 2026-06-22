use near_sdk::serde::{Deserialize, Serialize};
use near_sdk::{env, CurveType, PublicKey};

pub const FT_STORAGE_DEPOSIT_YOCTO: u128 = 1_250_000_000_000_000_000_000;
pub const NOT_ED25519: &str = "owner key must be ed25519";

pub fn ed25519_base58_or_panic(key: &PublicKey) -> String {
    ed25519_base58(key).unwrap_or_else(|| env::panic_str(NOT_ED25519))
}

pub fn panic_json<T: Serialize>(err: &T) -> ! {
    let json = near_sdk::serde_json::to_string(err)
        .unwrap_or_else(|_| String::from(r#"{"code":"serialization_failure"}"#));
    env::panic_str(&json)
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(crate = "near_sdk::serde")]
pub enum MintOutcome {
    CreationFailed,
    Active,
    SignerPending,
}

pub fn ed25519_base58(key: &PublicKey) -> Option<String> {
    if key.curve_type() != CurveType::ED25519 {
        return None;
    }
    key.to_string().strip_prefix("ed25519:").map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn strips_ed25519_prefix() {
        let key =
            PublicKey::from_str("ed25519:DcA2MzgpJbrUATQLLceocVckhhAqrkingax4oJ9kZ847").unwrap();
        let raw = ed25519_base58(&key).unwrap();
        assert!(!raw.contains(':'));
        assert_eq!(format!("ed25519:{raw}"), key.to_string());
    }

    #[test]
    fn rejects_non_ed25519() {
        let secp = PublicKey::from_str(
            "secp256k1:qMoRgcoXai4mBPsdbHi1wfyxF9TdbPCF4qSDQTRP3TfescSRoUdSx6nmeQoN3aiwGzwMyGXAb1gUjBTv5AY8DXj",
        )
        .unwrap();
        assert!(ed25519_base58(&secp).is_none());
    }
}
