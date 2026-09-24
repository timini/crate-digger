//! Where the app keeps service logins: always the OS credential store.

use std::sync::Arc;

use cd_connectors::credentials::{CachedStore, SecretStore};

/// Release builds use the keychain API directly; they are signed, so the
/// user's "Always Allow" sticks. macOS dev builds change on every rebuild,
/// so they reach the same keychain entries through Apple's `security` tool
/// instead, which avoids a permission prompt after each rebuild.
pub fn store() -> Arc<dyn SecretStore> {
    #[cfg(all(target_os = "macos", debug_assertions))]
    {
        Arc::new(CachedStore::new(cd_connectors::credentials::SecurityTool))
    }
    #[cfg(not(all(target_os = "macos", debug_assertions)))]
    {
        Arc::new(CachedStore::new(cd_connectors::credentials::Keychain))
    }
}
