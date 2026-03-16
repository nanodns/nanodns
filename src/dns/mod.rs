//! DNS packet handling — resolve local records, rewrite, or forward upstream.

pub mod resolver;
pub mod packet;
pub mod wildcard;

pub use resolver::Resolver;
