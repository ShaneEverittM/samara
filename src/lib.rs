#![doc = r#"
Samara is a Tokio + TEA runtime exploration library.

This crate currently contains runtime primitives for a thin-slice proof of concept:
- Open-set message routing via typed addresses and erased envelopes.
- Option A runtime loop (single mailbox) with supervised async effects.
- Example user actors/effects live in integration tests and examples, not in library modules.
"#]

pub mod prelude;
pub mod runtime;
pub mod system_effects;
