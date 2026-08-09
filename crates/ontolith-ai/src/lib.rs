//! AI-native semantic runtime extension (L8 / R4, Phase 8).
//!
//! P8-01 语义-向量桥接第一步：可插拔 [`EmbeddingProvider`]（树内确定性
//! feature-hash fallback，零新增外部依赖）+ 语义索引 + 余弦 top-k 检索。
//! 检索是近似召回，验证仍走 SPARQL/SHACL。

pub mod application;
pub mod domain;
pub mod infrastructure;

pub const CRATE_ID: &str = "ontolith-ai";

pub fn healthcheck() -> bool {
    true
}
