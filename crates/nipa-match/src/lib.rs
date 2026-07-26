//! nipa-match：弹弹play 文件识别客户端（L1 层，开发文档 §4.1）。
//!
//! 功能范围：
//! - **认证**（[`DandanAuth`]）：凭证模式（`X-AppId` + `X-AppSecret`，服务器端）/
//!   签名模式（`X-AppId` + `X-Timestamp` + `X-Signature`，
//!   `X-Signature = base64(sha256(AppId + Timestamp + Path + AppSecret))`）/
//!   无凭证（`None`——调用方应据此**整级跳过 L1、直接 L2**，见 §4.1 降级路径）；
//! - **match 客户端**（[`DandanClient`]）：`POST /api/v2/match`
//!   （`matchMode=hashAndFileName`）与 `POST /api/v2/match/batch`（≤32 个/次，
//!   超出自动分批串行，只回精确命中）；内置最小请求间隔节流（默认 300ms）；
//! - **hash**（[`dandan_hash_bytes`] / [`dandan_hash_reader`]）：文件前 16MB 的 MD5
//!   （不足 16MB 取全文件），hex 小写；
//! - **缓存语义三分类**（[`classify`] → [`MatchOutcome`]）：精确命中 / 候选 / 未匹配
//!   （"未匹配"由上层缓存 3 天，本 crate 不做存储）。
//!
//! API 细节调研：`docs/research/dandanplay-api.md`。
//! 合规红线（§4.1）：禁止批量抓取；弹幕功能必须免费；商用需授权。

pub mod auth;
pub mod client;
pub mod error;
pub mod hash;
pub mod secret;
pub mod types;

pub use auth::{DandanAuth, compute_signature};
pub use client::{BATCH_LIMIT, DEFAULT_BASE_URL, DEFAULT_MIN_INTERVAL, DandanClient};
pub use error::MatchError;
pub use hash::{DANDAN_HASH_SIZE, dandan_hash_bytes, dandan_hash_reader};
pub use secret::{NIPA_APP_ID, decode_secret, fetch_app_secret};
pub use types::{
    AnimeType, BatchItem, MatchMode, MatchOutcome, MatchRequest, MatchResponse, MatchResultV2,
    classify,
};
