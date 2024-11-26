use ckb_spawn::Spawn;
use std::future::Future;
use wasm_bindgen_futures::spawn_local;

#[derive(Debug, Clone)]
pub struct Handle {}

impl Handle {
    /// Spawns a future onto the runtime.
    ///
    /// This spawns the given future onto the runtime's executor
    pub fn spawn<F>(&self, future: F)
    where
        F: Future<Output = ()> + 'static,
    {
        spawn_local(async move { future.await })
    }
}

impl Spawn for Handle {
    fn spawn_task<F>(&self, future: F)
    where
        F: Future<Output = ()> + 'static,
    {
        self.spawn(future);
    }
}
