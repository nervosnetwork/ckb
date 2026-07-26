use super::*;

impl PrePool {
    pub(crate) fn mutate<T>(&self, apply: impl FnOnce(&mut PrePoolKernel) -> T) -> T {
        self.mutate_authoritative(apply)
    }

    pub(crate) fn mutate_lease<T>(
        &self,
        context: &'static str,
        apply: impl FnOnce(&mut PrePoolKernel) -> Result<T, PrePoolError>,
    ) -> Option<T> {
        match self.mutate_authoritative(apply) {
            Ok(value) => Some(value),
            Err(error) if error.is_stale_lease() => None,
            Err(error) => panic!("{context}: {error:?}"),
        }
    }
}
