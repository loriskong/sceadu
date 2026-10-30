use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use argon2::{Algorithm, Argon2, Params, Version};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use zeroize::Zeroize;

use crate::identity::{AgentId, Identity};
use crate::Error;

const KEYSTORE_VERSION: u32 = 1;
const AAD_PREFIX: &[u8] = b"sceadu-keystore-v1";
const KDF_NAME: &str = "argon2id";
const CIPHER_NAME: &str = "xchacha20poly1305";
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 24;
const KEY_LEN: usize = 32;
const SEED_LEN: usize = 32;

/// Default Argon2id memory cost in KiB (64 MiB).
pub const DEFAULT_M_COST: u32 = 65_536;
/// Default Argon2id iterations.
pub const DEFAULT_T_COST: u32 = 3;
/// Default Argon2id parallelism.
pub const DEFAULT_P_COST: u32 = 1;

/// Argon2id parameters stored in the keystore.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KdfParams {
    pub name: String,
    pub m_cost: u32,
    pub t_cost: u32,
    pub p_cost: u32,
    pub salt: String,
}

/// AEAD parameters stored in the keystore.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CipherParams {
    pub name: String,
    pub nonce: String,
}

/// On-disk keystore representation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KeystoreFile {
    pub version: u32,
    pub agent_id: AgentId,
    pub kdf: KdfParams,
    pub cipher: CipherParams,
    pub ciphertext: String,
}

/// Default keystore path: `$SCEADU_HOME/identity.json` or `$HOME/.sceadu/identity.json`.
pub fn default_path() -> Result<PathBuf, Error> {
    if let Some(home) = std::env::var_os("SCEADU_HOME") {
        if home.is_empty() {
            return Err(Error::HomeNotFound);
        }
        return Ok(PathBuf::from(home).join("identity.json"));
    }

    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .ok_or(Error::HomeNotFound)?;
    Ok(PathBuf::from(home).join(".sceadu").join("identity.json"))
}

/// Create a keystore with the default Argon2id parameters.
pub fn create(path: &Path, identity: &Identity, passphrase: &str) -> Result<(), Error> {
    create_with_params(
        path,
        identity,
        passphrase,
        DEFAULT_M_COST,
        DEFAULT_T_COST,
        DEFAULT_P_COST,
    )
}

/// Read the public AgentID from a keystore without unlocking it.
pub fn read_public(path: &Path) -> Result<AgentId, Error> {
    Ok(read_file(path)?.agent_id)
}

/// Unlock a keystore and reconstruct the signing identity.
pub fn unlock(path: &Path, passphrase: &str) -> Result<Identity, Error> {
    let file = read_file(path)?;
    if file.version != KEYSTORE_VERSION {
        return Err(Error::KeystoreVersion(file.version));
    }
    if file.kdf.name != KDF_NAME || file.cipher.name != CIPHER_NAME {
        return Err(Error::KeystoreFormat(
            "unsupported kdf or cipher".to_owned(),
        ));
    }

    let salt = STANDARD
        .decode(&file.kdf.salt)
        .map_err(|error| Error::KeystoreFormat(error.to_string()))?;
    let nonce = STANDARD
        .decode(&file.cipher.nonce)
        .map_err(|error| Error::KeystoreFormat(error.to_string()))?;
    let ciphertext = STANDARD
        .decode(&file.ciphertext)
        .map_err(|error| Error::KeystoreFormat(error.to_string()))?;

    if salt.len() != SALT_LEN || nonce.len() != NONCE_LEN {
        return Err(Error::KeystoreFormat(
            "invalid salt or nonce length".to_owned(),
        ));
    }

    let mut key = [0u8; KEY_LEN];
    derive_key(
        passphrase,
        &salt,
        file.kdf.m_cost,
        file.kdf.t_cost,
        file.kdf.p_cost,
        &mut key,
    )?;

    let cipher = XChaCha20Poly1305::new_from_slice(&key)
        .map_err(|_| Error::KeystoreFormat("invalid key length".to_owned()))?;
    let aad = aad(&file.agent_id);
    let decrypted = cipher.decrypt(
        XNonce::from_slice(&nonce),
        Payload {
            msg: &ciphertext,
            aad: &aad,
        },
    );
    key.zeroize();

    let mut plaintext = decrypted.map_err(|_| Error::KeystoreUnlockFailed)?;
    if plaintext.len() != SEED_LEN {
        plaintext.zeroize();
        return Err(Error::KeystoreUnlockFailed);
    }

    let mut seed = [0u8; SEED_LEN];
    seed.copy_from_slice(&plaintext);
    plaintext.zeroize();
    let identity = Identity::from_seed(seed);
    seed.zeroize();

    if identity.agent_id() != file.agent_id {
        return Err(Error::KeystoreUnlockFailed);
    }

    Ok(identity)
}

fn create_with_params(
    path: &Path,
    identity: &Identity,
    passphrase: &str,
    m_cost: u32,
    t_cost: u32,
    p_cost: u32,
) -> Result<(), Error> {
    if path.exists() {
        return Err(Error::KeystoreExists);
    }

    let mut salt = [0u8; SALT_LEN];
    OsRng.fill_bytes(&mut salt);
    let mut nonce = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce);

    let mut key = [0u8; KEY_LEN];
    derive_key(passphrase, &salt, m_cost, t_cost, p_cost, &mut key)?;

    let mut seed = identity.to_seed();
    let aad = aad(&identity.agent_id());
    let cipher = XChaCha20Poly1305::new_from_slice(&key)
        .map_err(|_| Error::KeystoreFormat("invalid key length".to_owned()))?;
    let encrypted = cipher.encrypt(
        XNonce::from_slice(&nonce),
        Payload {
            msg: &seed,
            aad: &aad,
        },
    );
    key.zeroize();
    seed.zeroize();

    let ciphertext = encrypted.map_err(|_| Error::KeystoreUnlockFailed)?;
    let file = KeystoreFile {
        version: KEYSTORE_VERSION,
        agent_id: identity.agent_id(),
        kdf: KdfParams {
            name: KDF_NAME.to_owned(),
            m_cost,
            t_cost,
            p_cost,
            salt: STANDARD.encode(salt),
        },
        cipher: CipherParams {
            name: CIPHER_NAME.to_owned(),
            nonce: STANDARD.encode(nonce),
        },
        ciphertext: STANDARD.encode(ciphertext),
    };

    write_file(path, &file)
}

fn derive_key(
    passphrase: &str,
    salt: &[u8],
    m_cost: u32,
    t_cost: u32,
    p_cost: u32,
    output: &mut [u8; KEY_LEN],
) -> Result<(), Error> {
    let params = Params::new(m_cost, t_cost, p_cost, Some(KEY_LEN))
        .map_err(|error| Error::KeystoreFormat(error.to_string()))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    argon
        .hash_password_into(passphrase.as_bytes(), salt, output)
        .map_err(|error| Error::KeystoreFormat(error.to_string()))
}

fn aad(agent_id: &AgentId) -> Vec<u8> {
    let mut aad = Vec::with_capacity(AAD_PREFIX.len() + 64);
    aad.extend_from_slice(AAD_PREFIX);
    aad.extend_from_slice(agent_id.to_string().as_bytes());
    aad
}

fn read_file(path: &Path) -> Result<KeystoreFile, Error> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(Error::KeystoreNotFound);
        }
        Err(error) => return Err(Error::Io(error)),
    };

    serde_json::from_str(&contents).map_err(|error| Error::KeystoreFormat(error.to_string()))
}

fn write_file(path: &Path, file: &KeystoreFile) -> Result<(), Error> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::KeystoreFormat("keystore path has no parent directory".to_owned()))?;

    if !parent.as_os_str().is_empty() {
        let existed = parent.exists();
        fs::create_dir_all(parent)?;
        #[cfg(unix)]
        if !existed {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        }
    }

    let json = serde_json::to_vec_pretty(file)
        .map_err(|error| Error::KeystoreFormat(error.to_string()))?;
    let mut temporary = NamedTempFile::new_in(if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    })?;
    temporary.write_all(&json)?;
    temporary.flush()?;
    temporary.as_file().sync_all()?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
    }

    temporary
        .persist(path)
        .map_err(|error| Error::Io(error.error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_identity(seed: u8) -> Identity {
        Identity::from_seed([seed; 32])
    }

    fn create_test_keystore(path: &Path, identity: &Identity, passphrase: &str) {
        create_with_params(path, identity, passphrase, 8_192, 1, 1).unwrap();
    }

    #[test]
    fn round_trips_identity() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("identity.json");
        let identity = test_identity(1);

        create_test_keystore(&path, &identity, "correct horse battery staple");
        let unlocked = unlock(&path, "correct horse battery staple").unwrap();

        assert_eq!(unlocked.agent_id(), identity.agent_id());
        assert_eq!(read_public(&path).unwrap(), identity.agent_id());
    }

    #[test]
    fn rejects_wrong_passphrase() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("identity.json");
        create_test_keystore(&path, &test_identity(2), "correct horse battery staple");

        assert!(matches!(
            unlock(&path, "wrong passphrase"),
            Err(Error::KeystoreUnlockFailed)
        ));
    }

    #[test]
    fn rejects_tampered_ciphertext() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("identity.json");
        create_test_keystore(&path, &test_identity(3), "correct horse battery staple");

        let mut file: KeystoreFile =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let mut ciphertext = STANDARD.decode(&file.ciphertext).unwrap();
        ciphertext[0] ^= 0x01;
        file.ciphertext = STANDARD.encode(ciphertext);
        fs::write(&path, serde_json::to_vec_pretty(&file).unwrap()).unwrap();

        assert!(matches!(
            unlock(&path, "correct horse battery staple"),
            Err(Error::KeystoreUnlockFailed)
        ));
    }

    #[test]
    fn rejects_tampered_agent_id() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("identity.json");
        create_test_keystore(&path, &test_identity(4), "correct horse battery staple");

        let mut file: KeystoreFile =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        file.agent_id = test_identity(5).agent_id();
        fs::write(&path, serde_json::to_vec_pretty(&file).unwrap()).unwrap();

        assert!(matches!(
            unlock(&path, "correct horse battery staple"),
            Err(Error::KeystoreUnlockFailed)
        ));
    }

    #[test]
    fn refuses_to_overwrite_existing_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("identity.json");
        let identity = test_identity(6);
        create_test_keystore(&path, &identity, "correct horse battery staple");

        assert!(matches!(
            create_test_keystore_result(&path, &identity, "correct horse battery staple"),
            Err(Error::KeystoreExists)
        ));
    }

    fn create_test_keystore_result(
        path: &Path,
        identity: &Identity,
        passphrase: &str,
    ) -> Result<(), Error> {
        create_with_params(path, identity, passphrase, 8_192, 1, 1)
    }

    #[cfg(unix)]
    #[test]
    fn sets_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("identity.json");
        create_test_keystore(&path, &test_identity(7), "correct horse battery staple");

        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
