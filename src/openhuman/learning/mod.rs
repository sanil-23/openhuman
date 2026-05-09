//! Agent self-learning subsystem.
//!
//! Post-turn hooks that reflect on completed turns, extract user preferences,
//! track tool effectiveness, and store learnings in the Memory backend.
//!
//! # Phase 1 additions (#566)
//!
//! - [`candidate`] — `LearningCandidate`, `FacetClass`, `CueFamily`, `EvidenceRef`,
//!   and the thread-safe ring-buffer [`candidate::Buffer`] that collects evidence
//!   from producers (Phase 2) for consumption by the stability detector (Phase 3).
//!
//! # Phase 2 additions (#566)
//!
//! - [`extract`] — producer modules: `signature` (email identity parser),
//!   `heuristics` (length-ratio + edit-window + correction-repeat detectors),
//!   `summary_facets` (structured facets from the LLM summariser).

pub mod candidate;
pub mod extract;
pub mod linkedin_enrichment;
pub mod prompt_sections;
pub mod reflection;
pub mod schemas;
pub mod tool_tracker;
pub mod transcript_ingest;
pub mod user_profile;

pub use candidate::{Buffer, CueFamily, EvidenceRef, FacetClass, LearningCandidate};
pub use prompt_sections::{LearnedContextSection, UserProfileSection};
pub use reflection::ReflectionHook;
pub use schemas::{
    all_learning_controller_schemas, all_learning_registered_controllers, learning_schemas,
};
pub use tool_tracker::ToolTrackerHook;
pub use user_profile::UserProfileHook;
