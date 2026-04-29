//! CKB Benches
//!
//! ```console
//! cd benches && cargo bench --features ci -- --test
//! ```
//!
//! Compare allocator memory behavior with:
//!
//! ```console
//! cargo bench --bench allocator_memory --features ci
//! cargo bench --bench allocator_memory --no-default-features --features "ci jemalloc"
//! ```
