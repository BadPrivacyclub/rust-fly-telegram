//! A software WebAuthn authenticator: I implement enough of the spec to register a passkey with
//! Telegram and sign in with it later, with the key living in the recovery vault instead of a device.

use anyhow::{bail, Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use base64::Engine;
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

const ES256: i64 = -7;
const FLAG_USER_PRESENT: u8 = 0x01;
const FLAG_USER_VERIFIED: u8 = 0x04;
const FLAG_BACKUP_ELIGIBLE: u8 = 0x08;
const FLAG_BACKED_UP: u8 = 0x10;
const FLAG_ATTESTED: u8 = 0x40;

/// Everything I need to sign in again; I keep it sealed inside the vault.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredCredential {
    pub credential_id: String,
    pub secret: String,
    pub user_handle: String,
    pub rp_id: String,
    pub origin: String,
    pub sign_count: u32,
}

pub struct Registration {
    pub credential: StoredCredential,
    pub client_data_json: String,
    pub attestation_object: Vec<u8>,
}

pub struct Assertion {
    pub client_data_json: String,
    pub authenticator_data: Vec<u8>,
    pub signature: Vec<u8>,
    pub user_handle: String,
}

/// The provider name I record for the keys this authenticator creates.
pub fn provider_name() -> String {
    format!("Fly-Telegram v.{}", crate::VERSION)
}

/// Telegram may wrap the options in `publicKey`, as `navigator.credentials` expects, so I unwrap them.
fn options_root(raw: &str) -> Result<Value> {
    let value: Value = serde_json::from_str(raw).context("passkey options are not JSON")?;
    Ok(match value.get("publicKey") {
        Some(inner) if inner.is_object() => inner.clone(),
        _ => value,
    })
}

fn origin_for(rp_id: &str) -> String {
    format!("https://{rp_id}")
}

pub fn register(options_json: &str) -> Result<Registration> {
    let options = options_root(options_json)?;
    let challenge = options["challenge"]
        .as_str()
        .context("registration options have no challenge")?
        .to_string();
    let rp_id = options["rp"]["id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .unwrap_or("telegram.org")
        .to_string();
    let user_handle = options["user"]["id"]
        .as_str()
        .context("registration options have no user id")?
        .to_string();
    if let Some(params) = options["pubKeyCredParams"].as_array() {
        let supported = params.iter().any(|p| p["alg"].as_i64() == Some(ES256));
        if !params.is_empty() && !supported {
            bail!("Telegram did not offer the ES256 algorithm this authenticator uses");
        }
    }

    let signing_key = SigningKey::random(&mut rand::rngs::OsRng);
    let mut credential_id = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut credential_id);
    let origin = origin_for(&rp_id);
    let client_data_json = client_data("webauthn.create", &challenge, &origin);

    let point = signing_key.verifying_key().to_encoded_point(false);
    let (x, y) = (
        point.x().context("public key x")?,
        point.y().context("public key y")?,
    );
    let mut cose = Vec::new();
    cbor_map_header(&mut cose, 5);
    cbor_int(&mut cose, 1);
    cbor_int(&mut cose, 2);
    cbor_int(&mut cose, 3);
    cbor_int(&mut cose, ES256);
    cbor_int(&mut cose, -1);
    cbor_int(&mut cose, 1);
    cbor_int(&mut cose, -2);
    cbor_bytes(&mut cose, x);
    cbor_int(&mut cose, -3);
    cbor_bytes(&mut cose, y);

    let mut auth_data = Vec::new();
    auth_data.extend_from_slice(&Sha256::digest(rp_id.as_bytes()));
    auth_data.push(
        FLAG_USER_PRESENT
            | FLAG_USER_VERIFIED
            | FLAG_BACKUP_ELIGIBLE
            | FLAG_BACKED_UP
            | FLAG_ATTESTED,
    );
    auth_data.extend_from_slice(&0u32.to_be_bytes());
    auth_data.extend_from_slice(&[0u8; 16]);
    auth_data.extend_from_slice(&(credential_id.len() as u16).to_be_bytes());
    auth_data.extend_from_slice(&credential_id);
    auth_data.extend_from_slice(&cose);

    let mut attestation_object = Vec::new();
    cbor_map_header(&mut attestation_object, 3);
    cbor_text(&mut attestation_object, "fmt");
    cbor_text(&mut attestation_object, "none");
    cbor_text(&mut attestation_object, "attStmt");
    cbor_map_header(&mut attestation_object, 0);
    cbor_text(&mut attestation_object, "authData");
    cbor_bytes(&mut attestation_object, &auth_data);

    Ok(Registration {
        credential: StoredCredential {
            credential_id: B64URL.encode(credential_id),
            secret: B64URL.encode(signing_key.to_bytes()),
            user_handle,
            rp_id,
            origin,
            sign_count: 0,
        },
        client_data_json,
        attestation_object,
    })
}

/// I sign a login challenge and move the sign counter forward so Telegram can spot cloned keys.
pub fn assert(credential: &mut StoredCredential, options_json: &str) -> Result<Assertion> {
    let options = options_root(options_json)?;
    let challenge = options["challenge"]
        .as_str()
        .context("login options have no challenge")?;
    if let Some(rp_id) = options["rpId"].as_str() {
        if !rp_id.is_empty() && rp_id != credential.rp_id {
            bail!(
                "the passkey belongs to {}, but Telegram asked for {rp_id}",
                credential.rp_id
            );
        }
    }
    if let Some(allowed) = options["allowCredentials"].as_array() {
        let listed = allowed
            .iter()
            .any(|entry| entry["id"].as_str() == Some(credential.credential_id.as_str()));
        if !allowed.is_empty() && !listed {
            bail!("Telegram does not list this passkey for the account any more");
        }
    }
    let secret = B64URL
        .decode(&credential.secret)
        .context("stored passkey secret is damaged")?;
    let signing_key =
        SigningKey::from_slice(&secret).context("stored passkey secret is invalid")?;
    credential.sign_count = credential.sign_count.wrapping_add(1);

    let client_data_json = client_data("webauthn.get", challenge, &credential.origin);
    let mut authenticator_data = Vec::new();
    authenticator_data.extend_from_slice(&Sha256::digest(credential.rp_id.as_bytes()));
    authenticator_data
        .push(FLAG_USER_PRESENT | FLAG_USER_VERIFIED | FLAG_BACKUP_ELIGIBLE | FLAG_BACKED_UP);
    authenticator_data.extend_from_slice(&credential.sign_count.to_be_bytes());

    let mut signed = authenticator_data.clone();
    signed.extend_from_slice(&Sha256::digest(client_data_json.as_bytes()));
    let signature: Signature = signing_key.sign(&signed);
    Ok(Assertion {
        client_data_json,
        authenticator_data,
        signature: signature.to_der().as_bytes().to_vec(),
        user_handle: credential.user_handle.clone(),
    })
}

fn client_data(kind: &str, challenge: &str, origin: &str) -> String {
    serde_json::json!({
        "type": kind,
        "challenge": challenge,
        "origin": origin,
        "crossOrigin": false,
    })
    .to_string()
}

fn cbor_head(out: &mut Vec<u8>, major: u8, value: u64) {
    let major = major << 5;
    match value {
        0..=23 => out.push(major | value as u8),
        24..=0xff => out.extend_from_slice(&[major | 24, value as u8]),
        0x100..=0xffff => {
            out.push(major | 25);
            out.extend_from_slice(&(value as u16).to_be_bytes());
        }
        0x1_0000..=0xffff_ffff => {
            out.push(major | 26);
            out.extend_from_slice(&(value as u32).to_be_bytes());
        }
        _ => {
            out.push(major | 27);
            out.extend_from_slice(&value.to_be_bytes());
        }
    }
}

fn cbor_int(out: &mut Vec<u8>, value: i64) {
    if value >= 0 {
        cbor_head(out, 0, value as u64);
    } else {
        cbor_head(out, 1, (-1 - value) as u64);
    }
}

fn cbor_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    cbor_head(out, 2, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

fn cbor_text(out: &mut Vec<u8>, text: &str) {
    cbor_head(out, 3, text.len() as u64);
    out.extend_from_slice(text.as_bytes());
}

fn cbor_map_header(out: &mut Vec<u8>, entries: u64) {
    cbor_head(out, 5, entries);
}

#[cfg(test)]
mod tests {
    use super::*;
    use p256::ecdsa::signature::Verifier;
    use p256::ecdsa::VerifyingKey;

    const OPTIONS: &str = r#"{"publicKey":{"challenge":"dGVzdC1jaGFsbGVuZ2U","rp":{"id":"telegram.org","name":"Telegram"},"user":{"id":"dXNlcg","name":"alex","displayName":"Alex"},"pubKeyCredParams":[{"type":"public-key","alg":-7}]}}"#;

    #[test]
    fn cbor_encodes_small_and_negative_numbers() {
        let mut out = Vec::new();
        cbor_int(&mut out, -7);
        cbor_int(&mut out, 24);
        cbor_int(&mut out, 500);
        assert_eq!(out, vec![0x26, 0x18, 24, 0x19, 0x01, 0xf4]);
    }

    #[test]
    fn registration_object_has_expected_layout() {
        let registration = register(OPTIONS).unwrap();
        let object = &registration.attestation_object;
        assert_eq!(object[0], 0xa3);
        assert!(object.windows(4).any(|w| w == b"none"));
        let client: Value = serde_json::from_str(&registration.client_data_json).unwrap();
        assert_eq!(client["type"], "webauthn.create");
        assert_eq!(client["challenge"], "dGVzdC1jaGFsbGVuZ2U");
        assert_eq!(client["origin"], "https://telegram.org");
        assert_eq!(registration.credential.user_handle, "dXNlcg");
    }

    #[test]
    fn assertion_signature_verifies() {
        let mut credential = register(OPTIONS).unwrap().credential;
        let login = r#"{"challenge":"bG9naW4","rpId":"telegram.org"}"#;
        let assertion = assert(&mut credential, login).unwrap();
        assert_eq!(credential.sign_count, 1);
        assert_eq!(&assertion.authenticator_data[33..37], &1u32.to_be_bytes());
        let secret = B64URL.decode(&credential.secret).unwrap();
        let key = SigningKey::from_slice(&secret).unwrap();
        let verifying = VerifyingKey::from(&key);
        let mut signed = assertion.authenticator_data.clone();
        signed.extend_from_slice(&Sha256::digest(assertion.client_data_json.as_bytes()));
        let signature = Signature::from_der(&assertion.signature).unwrap();
        assert!(verifying.verify(&signed, &signature).is_ok());
    }

    #[test]
    fn foreign_relying_party_is_refused() {
        let mut credential = register(OPTIONS).unwrap().credential;
        assert!(assert(
            &mut credential,
            r#"{"challenge":"x","rpId":"evil.example"}"#
        )
        .is_err());
    }
}
