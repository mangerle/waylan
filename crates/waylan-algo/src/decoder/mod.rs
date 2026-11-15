//! 统一束搜索解码模块 (`beam_v2`)
//!
//! # 设计原理
//! - **实现初衷**：以按字节位置索引的 `SyllableLattice` + DAT 游标词组匹配器 `DatPhraseMatcher`
//!   + 按位置同步束搜索 `PositionalBeamDecoder` 取代旧版多阶段 `SegmentPath` DFS 枚举与
//!     双层 `ViterbiDecoder` / `SpatialTypoCorrector` 外挂纠错架构。
//! - **核心优势**：全拼、模糊音、四类容错（最大 `max_typos = 2`）、无截断全拼混简拼与
//!   末尾半成品补全在同一束空间内直接竞争。
//! - **代价与局限**：单边最多携带两个音节，词组最大跨越 6 个音节。

pub mod alias;
pub mod beam;
pub mod beam_cache;
mod beam_finalize;
mod beam_scoring;
mod beam_types;
pub mod fsa;
pub mod fuzzy;
pub mod language_model;
pub mod lattice;
mod lattice_typo;
pub mod matcher;
mod mem_matcher;
pub mod weights;

pub use beam::{
    BeamDecodeOptions, DecodeResult, DecodedSentence, PositionalBeamDecoder, SentenceHeadSegment,
};
pub use beam_cache::{compute_decode_config_sig, IncrementalBeamCache};
pub use beam_scoring::{score_phrase_match, to_positive_score, unigram_log_score};
pub use fsa::PrismFsa;
pub use fuzzy::PrismFuzzy;
pub use language_model::BigramLanguageModel;
pub use lattice::{EdgeKind, LatticeOptions, SyllableEdge, SyllableLattice, TypoKind};
pub use matcher::{DatPhraseMatcher, MatcherOptions, PhraseMatch};
pub use weights::ScoringWeights;
