use cd_core::adapters::{AdapterError, AdapterResult};

/// A closed set prevents callers from reading unrelated keychain entries.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Credential {
    Llm,
    Discogs,
    Youtube,
    SoulseekUsername,
    SoulseekPassword,
    Slskd,
}

impl Credential {
    pub fn name(self) -> &'static str {
        match self {
            Self::Llm => "llm",
            Self::Discogs => "discogs",
            Self::Youtube => "youtube",
            Self::SoulseekUsername => "soulseek_username",
            Self::SoulseekPassword => "soulseek_password",
            Self::Slskd => "slskd",
        }
    }
}

pub trait SecretStore: Send + Sync {
    fn get(&self, key: Credential) -> AdapterResult<Option<String>>;
    fn set(&self, key: Credential, value: Option<&str>) -> AdapterResult<()>;
}

/// Matches the app identifier so entries are easy to find in Keychain Access.
pub const SERVICE: &str = "io.github.timini.cratedigger";

pub struct Keychain;

fn entry(key: Credential) -> AdapterResult<keyring::Entry> {
    keyring::Entry::new(SERVICE, key.name()).map_err(keychain_error)
}

// Native errors can contain account names. Report only a fixed actionable message.
fn keychain_error(_: keyring::Error) -> AdapterError {
    AdapterError::Unavailable(
        "Cannot access the operating system credential store. Unlock your keychain or enable a Secret Service session, then retry.".into(),
    )
}

impl SecretStore for Keychain {
    fn get(&self, key: Credential) -> AdapterResult<Option<String>> {
        match entry(key)?.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(keychain_error(e)),
        }
    }

    fn set(&self, key: Credential, value: Option<&str>) -> AdapterResult<()> {
        let entry = entry(key)?;
        match value.filter(|v| !v.is_empty()) {
            Some(value) => entry.set_password(value).map_err(keychain_error),
            None => match entry.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(e) => Err(keychain_error(e)),
            },
        }
    }
}

/// In-memory store for tests and for profiles that must not touch the keychain.
#[derive(Default)]
pub struct MemoryStore(std::sync::Mutex<std::collections::HashMap<Credential, String>>);

impl SecretStore for MemoryStore {
    fn get(&self, key: Credential) -> AdapterResult<Option<String>> {
        Ok(self.0.lock().unwrap().get(&key).cloned())
    }

    fn set(&self, key: Credential, value: Option<&str>) -> AdapterResult<()> {
        let mut map = self.0.lock().unwrap();
        match value.filter(|v| !v.is_empty()) {
            Some(value) => map.insert(key, value.to_string()),
            None => map.remove(&key),
        };
        Ok(())
    }
}
