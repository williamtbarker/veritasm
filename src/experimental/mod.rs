//! Research substrates that are not part of the stable assembly pipeline.
//!
//! APIs below this module may be exercised and falsified independently, but
//! their presence does not change the stable CLI, on-disk formats, output
//! schemas, or supported assembly k-mer range.

pub mod authenticated_pair_graph;
pub mod compacted_dbg;
pub mod evidence_reconstruction;
pub mod external_cdbg;
pub mod external_reduce;
pub mod external_run;
pub mod multik;
pub mod multik_bundle;
pub mod multik_pipeline;
pub mod pair_mapper;
pub mod pair_path;
pub mod partitioned_dbg;
pub mod quality_correction;
pub mod retention;
pub mod spool_external;
pub mod transition_witness;
pub mod wide_kmer;
