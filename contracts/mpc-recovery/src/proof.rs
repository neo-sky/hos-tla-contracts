use near_sdk::{env, AccountId, PublicKey};

const DOMAIN_REQUEST: u8 = 1;
const DOMAIN_VERDICT: u8 = 2;

pub fn request_message(
    contract: &AccountId,
    account: &AccountId,
    new_owner: &PublicKey,
    round: u64,
) -> Vec<u8> {
    let mut m = vec![DOMAIN_REQUEST];
    push_str(&mut m, contract.as_str());
    push_str(&mut m, account.as_str());
    m.extend_from_slice(new_owner.as_bytes());
    m.extend_from_slice(&round.to_le_bytes());
    m
}

pub fn verdict_message(
    contract: &AccountId,
    account: &AccountId,
    new_owner: &PublicKey,
    round: u64,
    silent: bool,
) -> Vec<u8> {
    let mut m = vec![DOMAIN_VERDICT];
    push_str(&mut m, contract.as_str());
    push_str(&mut m, account.as_str());
    m.extend_from_slice(new_owner.as_bytes());
    m.extend_from_slice(&round.to_le_bytes());
    m.push(silent as u8);
    m
}

pub fn verify(message: &[u8], signature: &[u8; 64], key: &PublicKey) -> bool {
    match ed25519_key(key) {
        Some(pubkey) => env::ed25519_verify(signature, message, &pubkey),
        None => false,
    }
}

pub fn verify_quorum(
    message: &[u8],
    signatures: &[(PublicKey, [u8; 64])],
    watchers: &[PublicKey],
    threshold: u32,
) -> bool {
    let mut counted: Vec<&PublicKey> = Vec::new();
    for (key, signature) in signatures {
        if !watchers.contains(key) || counted.contains(&key) {
            continue;
        }
        if verify(message, signature, key) {
            counted.push(key);
        }
    }
    counted.len() as u32 >= threshold
}

fn ed25519_key(key: &PublicKey) -> Option<[u8; 32]> {
    let bytes = key.as_bytes();
    if bytes.len() != 33 || bytes[0] != 0 {
        return None;
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes[1..]);
    Some(out)
}

fn push_str(m: &mut Vec<u8>, s: &str) {
    m.extend_from_slice(&(s.len() as u32).to_le_bytes());
    m.extend_from_slice(s.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use near_sdk::test_utils::VMContextBuilder;
    use near_sdk::{testing_env, CurveType};
    use rand::rngs::OsRng;

    fn keypair() -> (SigningKey, PublicKey) {
        let sk = SigningKey::generate(&mut OsRng);
        let pk = PublicKey::from_parts(CurveType::ED25519, sk.verifying_key().to_bytes().to_vec())
            .unwrap();
        (sk, pk)
    }

    fn sign(sk: &SigningKey, msg: &[u8]) -> [u8; 64] {
        sk.sign(msg).to_bytes()
    }

    fn ctx() {
        testing_env!(VMContextBuilder::new().build());
    }

    #[test]
    fn verify_accepts_valid_and_rejects_tampered() {
        ctx();
        let (sk, pk) = keypair();
        let msg = b"recover";
        let sig = sign(&sk, msg);
        assert!(verify(msg, &sig, &pk));
        assert!(!verify(b"other", &sig, &pk));
    }

    #[test]
    fn quorum_requires_threshold_distinct_valid_watcher_signatures() {
        ctx();
        let (sk1, w1) = keypair();
        let (sk2, w2) = keypair();
        let (sk3, w3) = keypair();
        let (sk_outsider, outsider) = keypair();
        let watchers = vec![w1.clone(), w2.clone(), w3.clone()];
        let msg = b"verdict";

        let two = vec![(w1.clone(), sign(&sk1, msg)), (w2.clone(), sign(&sk2, msg))];
        assert!(verify_quorum(msg, &two, &watchers, 2));
        assert!(!verify_quorum(msg, &two, &watchers, 3));

        let duplicate = vec![(w1.clone(), sign(&sk1, msg)), (w1.clone(), sign(&sk1, msg))];
        assert!(!verify_quorum(msg, &duplicate, &watchers, 2));

        let outsider_sig = vec![
            (w1.clone(), sign(&sk1, msg)),
            (outsider.clone(), sign(&sk_outsider, msg)),
        ];
        assert!(!verify_quorum(msg, &outsider_sig, &watchers, 2));

        let forged = vec![(w1.clone(), sign(&sk1, msg)), (w2.clone(), sign(&sk3, msg))];
        assert!(!verify_quorum(msg, &forged, &watchers, 2));
    }

    #[test]
    fn verdict_message_binds_new_owner() {
        use std::str::FromStr;
        ctx();
        let contract = AccountId::from_str("rec.testnet").unwrap();
        let account = AccountId::from_str("victim.testnet").unwrap();
        let (_, owner_a) = keypair();
        let (_, owner_b) = keypair();
        assert_ne!(
            verdict_message(&contract, &account, &owner_a, 0, true),
            verdict_message(&contract, &account, &owner_b, 0, true)
        );
    }

    #[test]
    fn request_and_verdict_messages_are_domain_separated() {
        use std::str::FromStr;
        ctx();
        let contract = AccountId::from_str("rec.testnet").unwrap();
        let account = AccountId::from_str("victim.testnet").unwrap();
        let (_, new_owner) = keypair();
        let req = request_message(&contract, &account, &new_owner, 0);
        let verd = verdict_message(&contract, &account, &new_owner, 0, true);
        assert_eq!(req[0], DOMAIN_REQUEST);
        assert_eq!(verd[0], DOMAIN_VERDICT);
        assert_ne!(req[0], verd[0]);
    }
}
