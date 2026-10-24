use std::env;

use env_logger::{Builder as LoggerBuilder, Target};
// use jieba_rs::Jieba;
// use simsearch::{SearchOptions, SimSearch};
// use strsim::damerau_levenshtein as a;
// use textdistance::nstr::damerau_levenshtein;
use tokio::runtime::Builder;
// use triple_accel::levenshtein::levenshtein_simd_k;

use kiko::web::server::start_app;

// Avoid musl's default allocator due to lackluster performance
// https://nickb.dev/blog/default-musl-allocator-considered-harmful-to-performance
// Following were commented out because turso 0.7.2 already uses mimalloc, and this line will cause a err: error: the `#[global_allocator]` in this crate conflicts with global allocator in: turso
// Detail: https://github.com/tursodatabase/turso/issues/9147
// #[cfg(target_env = "musl")]
// #[global_allocator]
// static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> Result<(), std::io::Error> {
    // dialogflow::web::t1();
    unsafe {
        env::set_var("RUST_LOG", "INFO");
    }
    let mut builder = LoggerBuilder::from_default_env();
    builder
        .target(Target::Stdout)
        .format_module_path(false)
        .format_target(false)
        .format_indent(None);
    builder.init();
Kiko
    let runtime = Builder::new_multi_thread()
        .worker_threads(std::thread::available_parallelism()?.get() + 5usize)
        .thread_name("Kiko")
        .thread_stack_size(3 * 1024 * 1024)
        .enable_io()
        .enable_time()
        .build()
        .unwrap();

    // let (sender, recv) = tokio::sync::oneshot::channel::<()>();
    // runtime.spawn(dialogflow::web::clean_expired_session(recv));
    runtime.block_on(start_app());

    Ok(())
}
