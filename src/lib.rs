//! VeritAsm: evidence-first deterministic de novo unitig assembly from short reads.
//!
//! `exact` in this crate means full encoded sequence identity and checked integer
//! counting under the recorded parser, quality, support, graph, and mapping rules.
//! It does not establish biological truth, identity, presence, or absence.

#![forbid(unsafe_code)]

pub mod audit;
pub mod bloom;
pub mod bundle;
pub mod compact;
pub mod config;
pub mod count;
pub mod dna;
pub mod error;
pub mod experimental;
pub mod fastx;
pub mod graph;
pub mod indexed_mapper;
pub mod input;
pub mod library_model;
pub mod model;
pub mod pairs;
pub mod pipeline;
pub mod report;
pub mod spool;
pub mod transform;
pub mod validation;

pub use config::{AssembleConfig, InputSpec, Limits, Profile, ScientificConfig, SupportUnit};
pub use error::{ErrorCode, Result, VeritasmError};
pub use pipeline::{assemble, RunOutcome};
