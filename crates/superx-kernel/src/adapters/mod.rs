//! Agent adapters — the per-agent halves of the capture engine.
//!
//! Each adapter owns everything about ONE agent's format (where its
//! data lives, how to discover sources, how to parse events into
//! `message` + `telemetry_stream` rows). The engine
//! ([`crate::capture`]) never names an agent; adding an adapter
//! touches zero engine code (BLUEPRINT.md §5).

pub mod claude_code;
