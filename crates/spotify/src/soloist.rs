//! Spotify Soloist as this service's sound (crate `nikgob-soloist`): its
//! key lives in the same credential store as the grants, and while it is
//! set up and paired the account has sound even when Spotify refuses
//! librespot's audio keys.

use std::path::PathBuf;
use std::sync::Arc;

use nikgob_soloist::{BoxFuture, Config, KeyStore, Soloist};

use crate::credentials::{Secret, Slot, Store};

/// The Soloist key in the credential store.
pub struct Keys(pub Store);

impl KeyStore for Keys {
    fn load(&self) -> BoxFuture<'_, Option<String>> {
        Box::pin(async move {
            match self.0.load(Slot::Soloist).await {
                Ok(Some(Secret::Soloist(key))) => Some(key),
                Ok(_) => None,
                Err(e) => {
                    log::warn!("spotify: cannot read the Soloist key: {e}");
                    None
                }
            }
        })
    }

    fn save(&self, key: String) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            let generation = self.0.generation(Slot::Soloist);
            self.0
                .save(Slot::Soloist, generation, Secret::Soloist(key))
                .await
                .map_err(|e| e.to_string())
        })
    }

    fn clear(&self) {
        self.0.clear(Slot::Soloist);
    }
}

pub fn start(store: Store, dir: PathBuf, user_agent: String, rt: tokio::runtime::Handle) -> Arc<Soloist> {
    Soloist::new(
        Config {
            root: dir,
            device_name: nikgob_soloist::default_device_name(),
            user_agent,
        },
        Arc::new(Keys(store)),
        rt,
    )
}
