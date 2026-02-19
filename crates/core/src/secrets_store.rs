use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use chacha20poly1305::aead::rand_core::{OsRng, RngCore};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize, Default)]
struct SecretBlob {
    nonce_b64: String,
    ciphertext_b64: String,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct SecretMap {
    values: BTreeMap<String, String>,
}

pub fn set(project: &str, key: &str, value: &str) -> Result<()> {
    let mut map = load_map(project)?;
    map.values.insert(key.to_string(), value.to_string());
    save_map(project, &map)
}

pub fn get(project: &str, key: &str) -> Result<Option<String>> {
    let map = load_map(project)?;
    Ok(map.values.get(key).cloned())
}

pub fn delete(project: &str, key: &str) -> Result<bool> {
    let mut map = load_map(project)?;
    let existed = map.values.remove(key).is_some();
    if existed {
        save_map(project, &map)?;
    }
    Ok(existed)
}

pub fn list(project: &str) -> Result<Vec<String>> {
    let map = load_map(project)?;
    Ok(map.values.keys().cloned().collect())
}

fn load_map(project: &str) -> Result<SecretMap> {
    let path = secret_file(project)?;
    if !path.exists() {
        return Ok(SecretMap::default());
    }

    let blob: SecretBlob = serde_json::from_str(
        &fs::read_to_string(&path)
            .with_context(|| format!("Failed to read secret file {:?}", path))?,
    )
    .with_context(|| format!("Failed to parse secret blob {:?}", path))?;

    decrypt_blob(&blob)
}

fn save_map(project: &str, map: &SecretMap) -> Result<()> {
    let path = secret_file(project)?;
    let blob = encrypt_map(map)?;
    fs::write(&path, serde_json::to_string_pretty(&blob)?).with_context(|| {
        format!(
            "Failed to write encrypted secret file {:?}",
            path.as_os_str()
        )
    })?;
    Ok(())
}

fn encrypt_map(map: &SecretMap) -> Result<SecretBlob> {
    let key = load_or_create_key()?;
    let cipher = XChaCha20Poly1305::new(Key::from_slice(&key));

    let mut nonce = [0u8; 24];
    OsRng.fill_bytes(&mut nonce);

    let plaintext = serde_json::to_vec(map)?;
    let ciphertext = cipher
        .encrypt(XNonce::from_slice(&nonce), plaintext.as_ref())
        .map_err(|_| anyhow::anyhow!("Failed to encrypt secrets"))?;

    Ok(SecretBlob {
        nonce_b64: B64.encode(nonce),
        ciphertext_b64: B64.encode(ciphertext),
    })
}

fn decrypt_blob(blob: &SecretBlob) -> Result<SecretMap> {
    if blob.nonce_b64.is_empty() && blob.ciphertext_b64.is_empty() {
        return Ok(SecretMap::default());
    }

    let key = load_or_create_key()?;
    let cipher = XChaCha20Poly1305::new(Key::from_slice(&key));

    let nonce = B64
        .decode(blob.nonce_b64.as_bytes())
        .context("Failed to decode secret nonce")?;
    let ciphertext = B64
        .decode(blob.ciphertext_b64.as_bytes())
        .context("Failed to decode secret ciphertext")?;

    let plaintext = cipher
        .decrypt(XNonce::from_slice(&nonce), ciphertext.as_ref())
        .map_err(|_| anyhow::anyhow!("Failed to decrypt secrets (key mismatch or corruption)"))?;

    let map: SecretMap = serde_json::from_slice(&plaintext).context("Failed to parse secrets")?;
    Ok(map)
}

fn secrets_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Failed to resolve home directory")?;
    let dir = home.join(".airstack").join("secrets");
    fs::create_dir_all(&dir).with_context(|| format!("Failed to create secrets dir {:?}", dir))?;
    Ok(dir)
}

fn key_file() -> Result<PathBuf> {
    Ok(secrets_dir()?.join("master.key"))
}

fn secret_file(project: &str) -> Result<PathBuf> {
    Ok(secrets_dir()?.join(format!("{}.secrets.enc", project)))
}

fn load_or_create_key() -> Result<[u8; 32]> {
    let path = key_file()?;

    if path.exists() {
        let bytes =
            fs::read(&path).with_context(|| format!("Failed to read key file {:?}", path))?;
        if bytes.len() != 32 {
            anyhow::bail!("Invalid key file length in {:?}", path);
        }
        let mut key = [0u8; 32];
        key.copy_from_slice(&bytes);
        return Ok(key);
    }

    let mut key = [0u8; 32];
    OsRng.fill_bytes(&mut key);
    fs::write(&path, key).with_context(|| format!("Failed to write key file {:?}", path))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .with_context(|| format!("Failed to chmod key file {:?}", path))?;
    }

    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::{decrypt_blob, encrypt_map, SecretMap};
    use std::collections::BTreeMap;

    #[test]
    fn encrypt_decrypt_round_trip() {
        let map = SecretMap {
            values: BTreeMap::from([("TOKEN".to_string(), "abc123".to_string())]),
        };
        let blob = encrypt_map(&map).expect("encrypt should succeed");
        let out = decrypt_blob(&blob).expect("decrypt should succeed");
        assert_eq!(out.values.get("TOKEN").unwrap(), "abc123");
    }
}
