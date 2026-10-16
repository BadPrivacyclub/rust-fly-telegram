use anyhow::{Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64, Engine};
use ring::{
    rand::SystemRandom,
    signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_ASN1_SIGNING},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use zeroize::Zeroize;

#[derive(Clone, Serialize, Deserialize)]
pub struct Credential {
    #[serde(default)]
    pub registered: bool,
    pub id: String,
    pub rp_id: String,
    pub user_handle: String,
    pub private_key: Vec<u8>,
    pub counter: u32,
}

impl Drop for Credential {
    fn drop(&mut self) {
        self.private_key.zeroize();
    }
}

fn options(value: &Value) -> &Value {
    value.get("publicKey").unwrap_or(value)
}

fn rp(value: &Value, registration: bool) -> Result<String> {
    let value = options(value);
    let id = if registration {
        value["rp"]["id"].as_str()
    } else {
        value["rpId"].as_str()
    }
    .context("passkey RP ID is missing")?;
    anyhow::ensure!(
        id == "telegram.org" || id.ends_with(".telegram.org"),
        "unexpected passkey relying party"
    );
    Ok(id.into())
}

fn client_data(value: &Value, rp_id: &str, registration: bool) -> Result<String> {
    let challenge = options(value)["challenge"]
        .as_str()
        .context("passkey challenge is missing")?;
    let decoded = B64
        .decode(challenge.trim_end_matches('='))
        .context("invalid passkey challenge")?;
    anyhow::ensure!(
        decoded.len() >= 16 && decoded.len() <= 4096,
        "invalid challenge length"
    );
    Ok(json!({"type": if registration { "webauthn.create" } else { "webauthn.get" },
        "challenge": B64.encode(decoded), "origin": format!("https://{rp_id}"), "crossOrigin": false}).to_string())
}

fn cbor_bytes(out: &mut Vec<u8>, value: &[u8]) {
    if value.len() < 24 {
        out.push(0x40 + value.len() as u8);
    } else if value.len() < 256 {
        out.extend([0x58, value.len() as u8]);
    } else {
        out.push(0x59);
        out.extend((value.len() as u16).to_be_bytes());
    }
    out.extend(value);
}

pub fn create(value: &Value) -> Result<Credential> {
    let value = options(value);
    anyhow::ensure!(
        value["pubKeyCredParams"]
            .as_array()
            .is_some_and(|v| v.iter().any(|p| p["alg"] == -7)),
        "Telegram did not offer ES256 credentials"
    );
    let rng = SystemRandom::new();
    let key = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng)
        .map_err(|_| anyhow::anyhow!("passkey generation failed"))?;
    let id = rand::random::<[u8; 32]>();
    let encoded = value["user"]["id"]
        .as_str()
        .context("missing user handle")?;
    let handle = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .or_else(|_| B64.decode(encoded.trim_end_matches('=')))
        .context("invalid user handle encoding")?;
    let user_handle = String::from_utf8(handle).context("invalid user handle")?;
    anyhow::ensure!(
        user_handle.split_once(':').is_some_and(|(dc, id)| dc
            .parse::<i32>()
            .is_ok_and(|n| (1..=5).contains(&n))
            && id.parse::<i64>().is_ok_and(|n| n > 0)),
        "invalid Telegram passkey user handle"
    );
    Ok(Credential {
        registered: false,
        id: B64.encode(id),
        rp_id: rp(value, true)?,
        user_handle,
        private_key: key.as_ref().to_vec(),
        counter: 0,
    })
}

pub fn registration(credential: &Credential, value: &Value) -> Result<(String, Vec<u8>)> {
    anyhow::ensure!(rp(value, true)? == credential.rp_id, "passkey RP changed");
    let key = EcdsaKeyPair::from_pkcs8(
        &ECDSA_P256_SHA256_ASN1_SIGNING,
        &credential.private_key,
        &SystemRandom::new(),
    )
    .map_err(|_| anyhow::anyhow!("invalid passkey"))?;
    let public = key.public_key().as_ref();
    anyhow::ensure!(
        public.len() == 65 && public[0] == 4,
        "invalid ES256 public key"
    );
    let mut cose = vec![0xa5, 0x01, 0x02, 0x03, 0x26, 0x20, 0x01, 0x21];
    cbor_bytes(&mut cose, &public[1..33]);
    cose.push(0x22);
    cbor_bytes(&mut cose, &public[33..65]);
    let raw_id = B64.decode(&credential.id)?;
    let mut auth = Sha256::digest(credential.rp_id.as_bytes()).to_vec();
    auth.push(0x45);
    auth.extend(0u32.to_be_bytes());
    auth.extend([0; 16]);
    auth.extend((raw_id.len() as u16).to_be_bytes());
    auth.extend(raw_id);
    auth.extend(cose);
    let mut attestation = b"\xa3\x63fmt\x64none\x67attStmt\xa0\x68authData".to_vec();
    cbor_bytes(&mut attestation, &auth);
    Ok((client_data(value, &credential.rp_id, true)?, attestation))
}

pub fn assertion(credential: &mut Credential, value: &Value) -> Result<(String, Vec<u8>, Vec<u8>)> {
    anyhow::ensure!(rp(value, false)? == credential.rp_id, "passkey RP changed");
    if let Some(allowed) = options(value)["allowCredentials"]
        .as_array()
        .filter(|v| !v.is_empty())
    {
        anyhow::ensure!(
            allowed.iter().any(|c| c["id"]
                .as_str()
                .is_some_and(|id| id.trim_end_matches('=') == credential.id.trim_end_matches('='))),
            "passkey is not allowed for this login"
        );
    }
    credential.counter = credential
        .counter
        .checked_add(1)
        .context("passkey counter exhausted")?;
    let client_data = client_data(value, &credential.rp_id, false)?;
    let mut authenticator = Sha256::digest(credential.rp_id.as_bytes()).to_vec();
    authenticator.push(0x05);
    authenticator.extend(credential.counter.to_be_bytes());
    let mut data = authenticator.clone();
    data.extend(Sha256::digest(client_data.as_bytes()));
    let rng = SystemRandom::new();
    let key = EcdsaKeyPair::from_pkcs8(
        &ECDSA_P256_SHA256_ASN1_SIGNING,
        &credential.private_key,
        &rng,
    )
    .map_err(|_| anyhow::anyhow!("invalid passkey"))?;
    let signature = key
        .sign(&rng, &data)
        .map_err(|_| anyhow::anyhow!("passkey signing failed"))?;
    Ok((client_data, authenticator, signature.as_ref().to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn assertions_are_signed_and_rp_bound() {
        let options = json!({"rp":{"id":"telegram.org"}, "user":{"id":B64.encode(b"2:123")}, "challenge":B64.encode([3;32]), "pubKeyCredParams":[{"alg":-7}]});
        let mut key = create(&options).unwrap();
        assert!(registration(&key, &options).unwrap().1.len() > 100);
        let request = json!({"rpId":"telegram.org", "challenge":B64.encode([5;32])});
        let (client, auth, signature) = assertion(&mut key, &request).unwrap();
        let pair = EcdsaKeyPair::from_pkcs8(
            &ECDSA_P256_SHA256_ASN1_SIGNING,
            &key.private_key,
            &SystemRandom::new(),
        )
        .unwrap();
        let mut data = auth;
        data.extend(Sha256::digest(client.as_bytes()));
        ring::signature::UnparsedPublicKey::new(
            &ring::signature::ECDSA_P256_SHA256_ASN1,
            pair.public_key(),
        )
        .verify(&data, &signature)
        .unwrap();
        assert_eq!(key.counter, 1);
        assert!(assertion(&mut key, &json!({"rpId":"attacker.org","challenge":"x"})).is_err());
    }
}
