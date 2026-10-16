use anyhow::{Context, Result};
use grammers_client::{tl, Client};
use num_bigint::BigUint;
use rand::RngCore;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

fn hash(parts: &[&[u8]]) -> Vec<u8> {
    let mut state = Sha256::new();
    for part in parts {
        state.update(part);
    }
    state.finalize().to_vec()
}

fn padded(n: &BigUint, length: usize) -> Result<Vec<u8>> {
    let raw = n.to_bytes_be();
    anyhow::ensure!(raw.len() <= length, "invalid SRP integer");
    let mut out = vec![0; length - raw.len()];
    out.extend(raw);
    Ok(out)
}

pub async fn check(client: &Client, password: &str) -> Result<()> {
    let tl::enums::account::Password::Password(parameters) = client
        .invoke(&tl::functions::account::GetPassword {})
        .await?;
    let Some(tl::enums::PasswordKdfAlgo::Sha256Sha256Pbkdf2Hmacsha512iter100000Sha256ModPow(algo)) =
        parameters.current_algo
    else {
        anyhow::bail!("unsupported Telegram password algorithm");
    };
    let p = BigUint::from_bytes_be(&algo.p);
    let known_prime = BigUint::parse_bytes(
        concat!(
            "C71CAEB9C6B1C9048E6C522F70F13F73980D40238E3E21C14934D037563D930F",
            "48198A0AA7C14058229493D22530F4DBFA336F6E0AC925139543AED44CCE7C37",
            "20FD51F69458705AC68CD4FE6B6B13ABDC9746512969328454F18FAF8C595F64",
            "2477FE96BB2A941D5BCD1D4AC8CC49880708FA9B378E3C4F3A9060BEE67CF9A4",
            "A4A695811051907E162753B56B0F6B410DBA74D8A84B2A14B3144E0EF1284754",
            "FD17ED950D5965B4B9DD46582DB1178D169C6BC465B0D6FF9CA3928FEF5B9AE4",
            "E418FC15E83EBEA0F87FA9FF5EED70050DED2849F47BF959D956850CE929851F",
            "0D8115F635B105EE2E4E15D04B2454BF6F4FADF034B10403119CD8E3B92FCC5B"
        )
        .as_bytes(),
        16,
    )
    .context("invalid built-in SRP prime")?;
    anyhow::ensure!(
        p == known_prime && [3, 4, 5, 7].contains(&algo.g),
        "unsupported SRP group"
    );
    let g = BigUint::from(algo.g as u32);
    let b_raw = parameters.srp_b.context("missing SRP challenge")?;
    let b = BigUint::from_bytes_be(&b_raw);
    let boundary = BigUint::from(1u32) << 1984;
    anyhow::ensure!(b > boundary && b < &p - &boundary, "unsafe SRP challenge");
    let h1 = Zeroizing::new(hash(&[&algo.salt1, password.as_bytes(), &algo.salt1]));
    let h2 = Zeroizing::new(hash(&[&algo.salt2, &h1, &algo.salt2]));
    let mut stretched = Zeroizing::new([0u8; 64]);
    ring::pbkdf2::derive(
        ring::pbkdf2::PBKDF2_HMAC_SHA512,
        std::num::NonZeroU32::new(100_000).unwrap(),
        &algo.salt1,
        &h2,
        stretched.as_mut(),
    );
    let x_bytes = Zeroizing::new(hash(&[&algo.salt2, stretched.as_ref(), &algo.salt2]));
    let x = BigUint::from_bytes_be(&x_bytes);
    let mut random = Zeroizing::new([0u8; 256]);
    rand::rngs::OsRng.fill_bytes(random.as_mut());
    let a = BigUint::from_bytes_be(random.as_ref());
    let ga = g.modpow(&a, &p);
    anyhow::ensure!(
        ga > boundary && ga < &p - &boundary,
        "unsafe SRP ephemeral key"
    );
    let a_pad = padded(&ga, 256)?;
    let b_pad = padded(&b, 256)?;
    let g_pad = padded(&g, 256)?;
    let k = BigUint::from_bytes_be(&hash(&[&algo.p, &g_pad]));
    let u = BigUint::from_bytes_be(&hash(&[&a_pad, &b_pad]));
    anyhow::ensure!(u != BigUint::from(0u32), "invalid SRP scrambling parameter");
    let base = (&b + &p - ((&k * g.modpow(&x, &p)) % &p)) % &p;
    let secret = base.modpow(&(a + u * x), &p);
    let key = Zeroizing::new(hash(&[&padded(&secret, 256)?]));
    let mut xor = hash(&[&algo.p]);
    for (byte, other) in xor.iter_mut().zip(hash(&[&g_pad])) {
        *byte ^= other;
    }
    let proof = hash(&[
        &xor,
        &hash(&[&algo.salt1]),
        &hash(&[&algo.salt2]),
        &a_pad,
        &b_pad,
        &key,
    ]);
    client
        .invoke(&tl::functions::auth::CheckPassword {
            password: tl::types::InputCheckPasswordSrp {
                srp_id: parameters.srp_id.context("missing SRP ID")?,
                a: a_pad,
                m1: proof,
            }
            .into(),
        })
        .await?;
    Ok(())
}
