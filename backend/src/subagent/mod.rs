//! Reusable sub-agent domain primitives.
//!
//! The legacy `subagent_gate` and `subagent_orchestrator` modules remain as
//! compatibility adapters while lifecycle, policy, protocol and telemetry
//! concerns are migrated behind this boundary.

pub(crate) mod api;
pub(crate) mod hook_composer;
pub(crate) mod lifecycle;
pub(crate) mod protocol;
pub(crate) mod read_only_tool;
pub(crate) mod rules;
pub(crate) mod telemetry;
