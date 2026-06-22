use near_sdk::serde::{Deserialize, Serialize};
use near_sdk::{CurveType, PublicKey};

pub const FT_STORAGE_DEPOSIT_YOCTO: u128 = 1_250_000_000_000_000_000_000;

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
