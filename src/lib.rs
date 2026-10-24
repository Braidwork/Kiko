// #[global_allocator]
// static ALLOC: snmalloc_rs::SnMalloc = snmalloc_rs::SnMalloc;

pub(crate) mod ai;
pub(crate) mod config;
pub(crate) mod external;
pub(crate) mod flow;
pub(crate) mod intent;
pub(crate) mod kb;
pub(crate) mod man;
pub(crate) mod repo;
pub(crate) mod result;
pub(crate) mod robot;
pub(crate) mod security;
// #[cfg(test)]
// pub mod test;
pub(crate) mod variable;
pub mod web;
