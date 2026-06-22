//! HTTP Basic auth credentials for private feeds (Phase 6a-iii-b).
//!
//! Real-world feeds (Substack-style token URLs, Patreon tiers) gate access
//! behind HTTP Basic auth. The password lives in the secret service
//! (libsecret, via `oo7`); the database stores only a username
//! (`shows.auth_user`) and an opaque lookup key (`shows.auth_pass_ref`), never
//! the password inline (spec §8).
//!
//! [`CredentialStore`] is an enum rather than a `dyn` trait so its async
//! methods stay object-safe-free: the `Secret` variant wraps the libsecret
//! keyring, and `InMemory` backs tests and headless environments without a
//! running secret service (the roadmap's "in-memory backend").

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::error::{FetchError, Result};

/// The `app` attribute every Conservatory keyring item carries, so a search is
/// scoped to this application's credentials.
const SECRET_APP: &str = "conservatory";

/// A username + password pair resolved for one HTTP Basic auth request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BasicAuth {
    pub user: String,
    pub password: String,
}

/// A store of feed passwords keyed by an opaque `auth_pass_ref` (the value
/// `shows.auth_pass_ref` holds). Cheap to clone (both variants are `Arc`-backed),
/// so it can be threaded into a concurrent refresh.
#[derive(Clone)]
pub enum CredentialStore {
    /// The freedesktop secret service / libsecret (oo7).
    Secret(Arc<oo7::Keyring>),
    /// An in-memory map, for tests and environments without a secret service.
    InMemory(Arc<Mutex<HashMap<String, String>>>),
}

impl CredentialStore {
    /// An empty in-memory store.
    pub fn in_memory() -> Self {
        CredentialStore::InMemory(Arc::new(Mutex::new(HashMap::new())))
    }

    /// Connect to the running secret service (libsecret / portal / file backend,
    /// whichever oo7 selects).
    pub async fn secret_service() -> Result<Self> {
        let keyring = oo7::Keyring::new()
            .await
            .map_err(|e| FetchError::Credentials(e.to_string()))?;
        Ok(CredentialStore::Secret(Arc::new(keyring)))
    }

    fn attrs(key: &str) -> HashMap<&str, &str> {
        HashMap::from([("app", SECRET_APP), ("ref", key)])
    }

    /// Store (or replace) a password under `key`.
    pub async fn set(&self, key: &str, password: &str) -> Result<()> {
        match self {
            CredentialStore::InMemory(map) => {
                map.lock()
                    .await
                    .insert(key.to_string(), password.to_string());
                Ok(())
            }
            CredentialStore::Secret(keyring) => {
                let label = format!("Conservatory feed credential ({key})");
                keyring
                    .create_item(&label, &Self::attrs(key), password.as_bytes(), true)
                    .await
                    .map_err(|e| FetchError::Credentials(e.to_string()))
            }
        }
    }

    /// Fetch the password stored under `key`, if any.
    pub async fn get(&self, key: &str) -> Result<Option<String>> {
        match self {
            CredentialStore::InMemory(map) => Ok(map.lock().await.get(key).cloned()),
            CredentialStore::Secret(keyring) => {
                let items = keyring
                    .search_items(&Self::attrs(key))
                    .await
                    .map_err(|e| FetchError::Credentials(e.to_string()))?;
                match items.first() {
                    Some(item) => {
                        let secret = item
                            .secret()
                            .await
                            .map_err(|e| FetchError::Credentials(e.to_string()))?;
                        Ok(Some(String::from_utf8_lossy(&secret).into_owned()))
                    }
                    None => Ok(None),
                }
            }
        }
    }

    /// Remove the password stored under `key`.
    pub async fn delete(&self, key: &str) -> Result<()> {
        match self {
            CredentialStore::InMemory(map) => {
                map.lock().await.remove(key);
                Ok(())
            }
            CredentialStore::Secret(keyring) => keyring
                .delete(&Self::attrs(key))
                .await
                .map_err(|e| FetchError::Credentials(e.to_string())),
        }
    }

    /// Resolve a show's stored credential into a [`BasicAuth`], requiring both a
    /// username and a stored password. Returns `None` when the show is anonymous
    /// or its password is missing.
    pub async fn resolve(
        &self,
        auth_user: Option<&str>,
        auth_pass_ref: Option<&str>,
    ) -> Result<Option<BasicAuth>> {
        let (user, key) = match (auth_user, auth_pass_ref) {
            (Some(u), Some(k)) => (u, k),
            _ => return Ok(None),
        };
        Ok(self.get(key).await?.map(|password| BasicAuth {
            user: user.to_string(),
            password,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn in_memory_round_trips() {
        let store = CredentialStore::in_memory();
        assert_eq!(store.get("feed-1").await.unwrap(), None);

        store.set("feed-1", "hunter2").await.unwrap();
        assert_eq!(
            store.get("feed-1").await.unwrap().as_deref(),
            Some("hunter2")
        );

        store.delete("feed-1").await.unwrap();
        assert_eq!(store.get("feed-1").await.unwrap(), None);
    }

    #[tokio::test]
    async fn resolve_requires_user_and_stored_password() {
        let store = CredentialStore::in_memory();
        store.set("k", "pw").await.unwrap();

        // Both present -> BasicAuth.
        let auth = store.resolve(Some("alice"), Some("k")).await.unwrap();
        assert_eq!(
            auth,
            Some(BasicAuth {
                user: "alice".to_string(),
                password: "pw".to_string()
            })
        );

        // Missing username, or no stored password, -> None.
        assert_eq!(store.resolve(None, Some("k")).await.unwrap(), None);
        assert_eq!(
            store.resolve(Some("alice"), Some("absent")).await.unwrap(),
            None
        );
        assert_eq!(store.resolve(Some("alice"), None).await.unwrap(), None);
    }
}
