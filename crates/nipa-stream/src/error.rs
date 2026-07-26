//! nipa-stream 错误类型。
//!
//! sidecar 子进程模式（§2.3/§6.3）下的失败面：进程无法启动、非零退出、
//! 输出解析失败、字幕轨无文本可抽（图形字幕，§4.2 evidence 标注 unavailable）。

use std::path::PathBuf;

use thiserror::Error;

/// nipa-stream 统一错误。
#[derive(Debug, Error)]
pub enum StreamError {
    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Other(String),
    /// 子进程无法启动（二进制缺失/无执行权限）。
    #[error("无法启动 {program}：{source}")]
    Spawn {
        program: String,
        #[source]
        source: std::io::Error,
    },

    /// 子进程非零退出。
    #[error("{program} 退出异常（{status}）：{stderr}")]
    CommandFailed {
        program: String,
        status: String,
        stderr: String,
    },

    /// ffprobe 输出不是合法 JSON。
    #[error("ffprobe 输出解析失败：{0}")]
    Parse(#[from] serde_json::Error),

    /// 目标字幕轨是图形字幕（PGS/VobSub 等），无文本可抽取（§4.2：v1 不做 OCR）。
    #[error("流 0:{stream_index} 是图形字幕（PGS/VobSub 等位图格式），无文本可抽取（v1 不做 OCR）")]
    GraphicSubtitle { stream_index: u32 },

    /// 媒体文件不存在或不是常规文件。
    #[error("媒体文件不存在或不可读：{0}")]
    MediaNotFound(PathBuf),
}
