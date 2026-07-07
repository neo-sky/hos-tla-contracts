use hos_common::tx::write_str;

const TAG: u32 = 2_147_484_061;

pub fn payload(
    message: &str,
    nonce: &[u8; 32],
    recipient: &str,
    callback_url: Option<&str>,
) -> Vec<u8> {
    let capacity = 45 + message.len() + recipient.len() + callback_url.map_or(0, str::len);
    let mut b = Vec::with_capacity(capacity);
    b.extend_from_slice(&TAG.to_le_bytes());
    write_str(&mut b, message);
    b.extend_from_slice(nonce);
    write_str(&mut b, recipient);
    match callback_url {
        Some(url) => {
            b.push(1);
            write_str(&mut b, url);
        }
        None => b.push(0),
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use hos_common::tx::to_hex;

    #[test]
    fn payload_matches_nep413_borsh_layout() {
        let bytes = payload("hi", &[0u8; 32], "app.example.com", None);
        let expected = concat!(
            "9d010080",
            "020000006869",
            "0000000000000000000000000000000000000000000000000000000000000000",
            "0f0000006170702e6578616d706c652e636f6d",
            "00",
        );
        assert_eq!(to_hex(&bytes), expected);
    }

    #[test]
    fn payload_encodes_callback_url_option() {
        let bytes = payload(
            "hi",
            &[7u8; 32],
            "app.example.com",
            Some("https://x.example"),
        );
        let tail = "011100000068747470733a2f2f782e6578616d706c65";
        assert!(to_hex(&bytes).ends_with(tail));
        assert_eq!(&to_hex(&bytes)[..8], "9d010080");
    }
}
