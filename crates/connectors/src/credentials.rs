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

/// Reads each secret from the inner store at most once and keeps it in
/// memory for the life of the app, so the operating system asks for
/// keychain permission once per entry per launch rather than on every use.
/// Failed reads are not cached, so a denied prompt can be retried.
pub struct CachedStore<S> {
    inner: S,
    cache: std::sync::Mutex<std::collections::HashMap<Credential, Option<String>>>,
}

impl<S: SecretStore> CachedStore<S> {
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            cache: std::sync::Mutex::default(),
        }
    }
}

impl<S: SecretStore> SecretStore for CachedStore<S> {
    fn get(&self, key: Credential) -> AdapterResult<Option<String>> {
        if let Some(v) = self.cache.lock().unwrap().get(&key) {
            return Ok(v.clone());
        }
        let v = self.inner.get(key)?;
        self.cache.lock().unwrap().insert(key, v.clone());
        Ok(v)
    }

    fn set(&self, key: Credential, value: Option<&str>) -> AdapterResult<()> {
        self.inner.set(key, value)?;
        let value = value.filter(|v| !v.is_empty()).map(str::to_string);
        self.cache.lock().unwrap().insert(key, value);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[derive(Default)]
    struct Counting {
        inner: MemoryStore,
        reads: AtomicUsize,
    }

    impl SecretStore for Counting {
        fn get(&self, key: Credential) -> AdapterResult<Option<String>> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            self.inner.get(key)
        }
        fn set(&self, key: Credential, value: Option<&str>) -> AdapterResult<()> {
            self.inner.set(key, value)
        }
    }

    #[test]
    fn each_secret_is_read_from_the_keychain_once() {
        let store = CachedStore::new(Counting::default());
        store.inner.inner.set(Credential::Discogs, Some("t")).unwrap();
        for _ in 0..5 {
            assert_eq!(store.get(Credential::Discogs).unwrap().as_deref(), Some("t"));
            assert_eq!(store.get(Credential::Youtube).unwrap(), None);
        }
        assert_eq!(store.inner.reads.load(Ordering::SeqCst), 2);
        store.set(Credential::Youtube, Some("k")).unwrap();
        assert_eq!(store.get(Credential::Youtube).unwrap().as_deref(), Some("k"));
        store.set(Credential::Youtube, None).unwrap();
        assert_eq!(store.get(Credential::Youtube).unwrap(), None);
        assert_eq!(store.inner.reads.load(Ordering::SeqCst), 2);
    }
}
