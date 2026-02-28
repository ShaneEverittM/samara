use samara::runtime::{EffectFuture, Envelope, IssuedCmd};

use crate::support::{
    counter::{CounterMsg, PersistCmd},
    store::InMemoryStore,
};

#[derive(Clone)]
pub struct PersistenceEffects {
    store: InMemoryStore,
}

impl PersistenceEffects {
    pub fn new(store: InMemoryStore) -> Self {
        Self { store }
    }

    pub fn handle(&self, issued: IssuedCmd<PersistCmd>) -> EffectFuture {
        let store = self.store.clone();
        Box::pin(async move {
            let to = issued.origin;
            match issued.cmd {
                PersistCmd::PersistCount(value) => {
                    if value == u64::MAX {
                        let env = Envelope::with_meta(
                            to,
                            CounterMsg::PersistFailed("simulated persist failure".to_string()),
                            issued.meta,
                        );
                        Ok(vec![env])
                    } else {
                        store.push(value).await;
                        let env = Envelope::with_meta(to, CounterMsg::Persisted(value), issued.meta);
                        Ok(vec![env])
                    }
                }
            }
        })
    }
}
