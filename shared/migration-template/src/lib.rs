//! Provide proc-macros to setup migration.

extern crate proc_macro;

use proc_macro::TokenStream;
use quote::quote;
use syn::parse_macro_input;

/// multi thread migration template
#[proc_macro]
pub fn multi_thread_migration(input: TokenStream) -> TokenStream {
    let block_expr = parse_macro_input!(input as syn::ExprBlock);
    let expanded = quote! {
        const MAX_THREAD: u64 = 6;
        const MIN_THREAD: u64 = 2;
        const BATCH: usize = 1_000;

        let chain_db = ChainDB::new(db, StoreConfig::default());
        let tip = chain_db.get_tip_header().expect("db tip header index");
        let tip_number = tip.number();

        let tb_num = std::cmp::max(MIN_THREAD, num_cpus::get() as u64);
        let tb_num = std::cmp::min(tb_num, MAX_THREAD);
        let chunk_size = tip_number / tb_num;
        let remainder = tip_number % tb_num;
        let _barrier = ::std::sync::Arc::new(::std::sync::Barrier::new(tb_num as usize));

        let handles: Vec<_> = (0..tb_num).map(|i| {
            let chain_db = chain_db.clone();
            let pb = ::std::sync::Arc::clone(&pb);
            let barrier = Arc::clone(&_barrier);

            let last = i == (tb_num - 1);
            let size = if last {
                chunk_size + remainder
            } else {
                chunk_size
            };
            let end = if last {
                tip_number + 1
            } else {
                (i + 1) * chunk_size
            };

            let pbi = pb(size * 2);
            pbi.set_style(
                ProgressStyle::default_bar()
                    .template(
                        "{prefix:.bold.dim} {spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta}) {msg}",
                    )
                    .progress_chars("#>-"),
            );
            pbi.set_position(0);
            pbi.enable_steady_tick(5000);
            ::std::thread::spawn(move || {
                let mut wb = chain_db.new_write_batch();

                #block_expr

                if !wb.is_empty() {
                    chain_db.write(&wb).unwrap();
                }
                pbi.finish_with_message("done!");
            })
        }).collect();

        // Wait for other threads to finish.
        for handle in handles {
            handle.join().unwrap();
        }
        Ok(chain_db.into_inner())
    };

    TokenStream::from(expanded)
}
