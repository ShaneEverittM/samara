use std::sync::Arc;

use tokio::sync::Mutex;

#[derive(Clone, Default)]
pub struct InMemoryStore {
    persisted: Arc<Mutex<Vec<u64>>>,
}

impl InMemoryStore {
    pub async fn push(&self, value: u64) {
        self.persisted.lock().await.push(value);
    }

    pub async fn snapshot(&self) -> Vec<u64> {
        self.persisted.lock().await.clone()
    }
}
