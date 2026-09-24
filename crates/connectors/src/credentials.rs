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

/// The macOS keychain through Apple's `security` tool, for dev builds.
/// Keychain entries trust the program that created them; dev builds change
/// on every rebuild, so going through the keychain API asks for permission
/// again and again. `security` is a fixed, Apple-signed program, so entries
/// it created open without prompts. Secrets stay in the keychain.
#[cfg(target_os = "macos")]
pub struct SecurityTool;

#[cfg(target_os = "macos")]
impl SecretStore for SecurityTool {
    fn get(&self, key: Credential) -> AdapterResult<Option<String>> {
        let out = std::process::Command::new("/usr/bin/security")
            .args(["find-generic-password", "-s", SERVICE, "-a", key.name(), "-w"])
            .output()
            .map_err(|_| keychain_unavailable())?;
        match out.status.code() {
            Some(0) => {
                let v = String::from_utf8_lossy(&out.stdout)
                    .trim_end_matches('\n')
                    .to_string();
                Ok(Some(v).filter(|v| !v.is_empty()))
            }
            // 44: the item could not be found.
            Some(44) => Ok(None),
            _ => Err(keychain_unavailable()),
        }
    }

    fn set(&self, key: Credential, value: Option<&str>) -> AdapterResult<()> {
        let status = match value.filter(|v| !v.is_empty()) {
            Some(v) => std::process::Command::new("/usr/bin/security")
                .args([
                    "add-generic-password",
                    "-U",
                    "-s",
                    SERVICE,
                    "-a",
                    key.name(),
                    "-w",
                    v,
                ])
                .output(),
            None => std::process::Command::new("/usr/bin/security")
                .args(["delete-generic-password", "-s", SERVICE, "-a", key.name()])
                .output(),
        }
        .map_err(|_| keychain_unavailable())?
        .status
        .code();
        match (status, value.is_some()) {
            (Some(0), _) | (Some(44), false) => Ok(()),
            _ => Err(keychain_unavailable()),
        }
    }
}

#[cfg(target_os = "macos")]
fn keychain_unavailable() -> AdapterError {
    AdapterError::Unavailable("Cannot access the macOS keychain. Unlock it and retry.".into())
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
