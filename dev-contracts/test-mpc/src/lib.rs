use ed25519_dalek::{Signer, SigningKey};
use near_sdk::serde_json::{json, Value};
use near_sdk::{env, near, AccountId, PanicOnDefault, PublicKey};

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct TestMpc {
    secret: [u8; 32],
}

#[near]
impl TestMpc {
    #[init]
    pub fn new(secret: [u8; 32]) -> Self {
        Self { secret }
    }

    #[allow(unused_variables)]
    pub fn derived_public_key(
        &self,
        path: String,
        predecessor: AccountId,
        domain_id: u64,
    ) -> PublicKey {
        let verifying = SigningKey::from_bytes(&self.secret).verifying_key();
        let mut data = Vec::with_capacity(33);
        data.push(0);
        data.extend_from_slice(verifying.as_bytes());
        PublicKey::try_from(data).unwrap_or_else(|_| env::panic_str("bad derived key"))
    }

    #[payable]
    pub fn sign(&mut self, request: Value) -> Value {
        let payload_hex = request["payload_v2"]["Eddsa"]
            .as_str()
            .unwrap_or_else(|| env::panic_str("missing Eddsa payload"));
        let payload = from_hex(payload_hex);
        let signature = SigningKey::from_bytes(&self.secret).sign(&payload);
        json!({ "scheme": "Ed25519", "signature": signature.to_bytes().to_vec() })
    }
}

fn from_hex(s: &str) -> Vec<u8> {
    if s.len() % 2 != 0 {
        env::panic_str("odd hex length");
    }
    s.as_bytes()
        .chunks(2)
        .map(|pair| {
            let hi = nibble(pair[0]);
            let lo = nibble(pair[1]);
            (hi << 4) | lo
        })
        .collect()
}

fn nibble(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        b'A'..=b'F' => c - b'A' + 10,
        _ => env::panic_str("invalid hex"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use near_sdk::test_utils::VMContextBuilder;
    use near_sdk::testing_env;
    use std::str::FromStr;

    fn ctx() {
        testing_env!(VMContextBuilder::new()
            .current_account_id(AccountId::from_str("mpc.testnet").unwrap())
            .predecessor_account_id(AccountId::from_str("caller.testnet").unwrap())
            .build());
    }

    #[test]
    fn derived_key_is_stable_ed25519() {
        ctx();
        let c = TestMpc::new([7u8; 32]);
        let a = c.derived_public_key("p".into(), AccountId::from_str("x.testnet").unwrap(), 1);
        let b = c.derived_public_key("q".into(), AccountId::from_str("y.testnet").unwrap(), 1);
        assert_eq!(a, b);
        assert_eq!(a.as_bytes()[0], 0);
    }

    #[test]
    fn sign_produces_verifiable_signature() {
        ctx();
        let mut c = TestMpc::new([7u8; 32]);
        let out = c.sign(json!({
            "path": "p",
            "payload_v2": { "Eddsa": "00ff10" },
            "domain_id": 1,
        }));
        assert_eq!(out["scheme"], "Ed25519");
        let sig_bytes: Vec<u8> = out["signature"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap() as u8)
            .collect();
        let signature = ed25519_dalek::Signature::from_bytes(&sig_bytes.try_into().unwrap());
        let verifying = SigningKey::from_bytes(&[7u8; 32]).verifying_key();
        assert!(verifying
            .verify_strict(&[0x00, 0xff, 0x10], &signature)
            .is_ok());
    }

    #[test]
    #[should_panic(expected = "invalid hex")]
    fn sign_rejects_bad_hex() {
        ctx();
        let mut c = TestMpc::new([7u8; 32]);
        let _ = c.sign(json!({ "payload_v2": { "Eddsa": "zz" }, "domain_id": 1 }));
    }
}
