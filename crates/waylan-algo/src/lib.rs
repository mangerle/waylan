pub mod bigram;
#[cfg(any(feature = "dict-builder", test))]
pub mod builder;
pub(crate) mod compact_writer;
pub mod composition;
pub mod custom_phrase;
pub mod dat;
pub(crate) mod dat_builder;
pub mod decoder;
pub mod dispatcher;
pub mod engine;
pub mod entity;
pub mod error;
#[cfg(any(test, feature = "eval-bench"))]
pub mod eval_bench;
pub mod evaluator;
mod fs_guard;
pub mod opencc;
pub mod pipeline;
pub mod syllables;
pub mod table;
pub mod types;
pub mod user_adaptive;
pub mod vocab;

pub use bigram::CompactMmapBigram;
pub use composition::Composition;
pub use custom_phrase::{
    AuxiliaryDict, EnglishSuggestionManager, SymbolSuggestionManager, load_auxiliary_managers,
};
pub use decoder::ScoringWeights;
pub use engine::AlgoEngine;
pub use entity::EntityProtectionManager;
pub use opencc::OpenCcDict;
pub use table::{
    CompactMmapLexicon, LENGTH_BOOST_MULTIPLIER, LexiconTable, calculate_length_boost,
    extract_raw_score,
};
pub use types::{AlgoCandidate, AlgoConfig, CandidateTier};
pub use user_adaptive::{UserAdaptiveModel, UserAdaptiveWal};
pub use vocab::Vocabulary;
