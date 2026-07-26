//! 服务器配置（开发文档 §11：TOML 配置文件，优先级 env > file > default）。
//!
//! 加载流程由 nipa-server 驱动：读 `NIPA_CONFIG` 指定路径（默认 ./nipaserver.toml），
//! 缺文件用默认值，最后套 env 覆盖。本模块只提供类型与纯逻辑，不做 IO 之外的事。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 默认监听端口（§8.2：避开客户端 1180，定为 11810）。
pub const DEFAULT_PORT: u16 = 11810;

/// 服务器配置根。所有字段带默认值，缺配置文件时整体可用。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    pub server: ServerSection,
    pub log: LogSection,
    /// AI 刮削模型配置（§2.2）。缺省时 L2 刮削不可用（capabilities.ai_scrape=false）。
    pub model: ModelSection,
    /// 元数据源配置（§5）。
    pub providers: ProvidersSection,
    // TODO(M1): [libraries] 首启向导写入；[dandanplay] 认证配置。
}

/// `[providers]` 段：元数据源凭证（§5）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProvidersSection {
    /// TMDB API Read Access Token（Bearer）。空 = TMDB 工具不可用，仅 Bangumi。
    /// TODO: 注册 NipaPlay 项目级 key 内置为默认（Jellyfin 模式，§5）。
    pub tmdb_token: String,
    /// Bangumi API User-Agent 覆盖；空用内置默认（AimesSoft/nipaserver/...）。
    pub bangumi_user_agent: String,
    /// 弹弹play L1 开关（§4.1）。默认 true：启动时从分发服务器拉 appSecret，
    /// 失败自动降级 L2-only。false = 强制跳过 L1。
    #[serde(default = "default_true")]
    pub dandanplay_l1: bool,
}

fn default_true() -> bool {
    true
}

impl Default for ProvidersSection {
    fn default() -> Self {
        Self {
            tmdb_token: String::new(),
            bangumi_user_agent: String::new(),
            dandanplay_l1: true,
        }
    }
}

/// `[model]` 段：OpenAI 兼容端点三要素 + 护栏（对应 nipa-agent 的
/// ModelProviderInfo/AgentConfig，字段语义见 docs/03-agent接口契约.md）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ModelSection {
    /// 形如 https://api.deepseek.com/v1（不含 /chat/completions）。
    pub base_url: String,
    /// 内联 key；与 api_key_env 二选一，内联优先。
    pub api_key: String,
    /// 从环境变量读 key（避免 key 落配置文件）。
    pub api_key_env: String,
    /// 模型名，如 deepseek-chat / gemini-3-flash。
    pub model: String,
    /// 每任务轮数上限；0 取 nipa-agent 默认（16）。
    pub max_rounds: u32,
    /// 每任务 token 预算；0 表示不限。
    pub max_total_tokens: u64,
}

impl ModelSection {
    /// L2 刮削是否已配置可用。
    pub fn is_configured(&self) -> bool {
        !self.base_url.trim().is_empty() && !self.model.trim().is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerSection {
    /// 监听地址。
    pub bind: String,
    /// 监听端口（env 覆盖：NIPA_PORT）。
    pub port: u16,
    /// 数据目录：SQLite 数据库、图片缓存等（env 覆盖：NIPA_DATA_DIR）。
    pub data_dir: PathBuf,
}

impl Default for ServerSection {
    fn default() -> Self {
        Self {
            bind: "0.0.0.0".to_string(),
            port: DEFAULT_PORT,
            data_dir: PathBuf::from("./data"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LogSection {
    /// tracing env-filter 语法；RUST_LOG 存在时以 RUST_LOG 优先。
    pub filter: String,
    // TODO(§11): 分级滚动日志文件（WebUI 日志页数据源）。
}

impl Default for LogSection {
    fn default() -> Self {
        Self {
            filter: "info".to_string(),
        }
    }
}

/// 配置加载错误。
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("读取配置文件 {path} 失败: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("解析配置文件 {path} 失败: {source}")]
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("环境变量 {name} 的值无效: {value}")]
    InvalidEnv { name: String, value: String },
}

impl ServerConfig {
    /// 解析 TOML 文本。
    pub fn from_toml_str(text: &str, path: &std::path::Path) -> Result<Self, ConfigError> {
        toml::from_str(text).map_err(|source| ConfigError::Parse {
            path: path.to_path_buf(),
            source,
        })
    }

    /// 从文件加载；文件不存在时返回默认配置（`Ok(None)` 语义并入：以 bool 标示是否命中文件）。
    pub fn load_file(path: &std::path::Path) -> Result<(Self, bool), ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok((Self::from_toml_str(&text, path)?, true)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((Self::default(), false)),
            Err(source) => Err(ConfigError::Read {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    /// 应用环境变量覆盖（优先级 env > file > default）。
    ///
    /// v1 覆盖集：`NIPA_BIND`、`NIPA_PORT`、`NIPA_DATA_DIR`。
    /// TODO: 后续扩展 NIPA_LOG 等。
    pub fn apply_env_overrides(&mut self) -> Result<(), ConfigError> {
        if let Ok(bind) = std::env::var("NIPA_BIND") {
            self.server.bind = bind;
        }
        if let Ok(port) = std::env::var("NIPA_PORT") {
            self.server.port = port.parse().map_err(|_| ConfigError::InvalidEnv {
                name: "NIPA_PORT".to_string(),
                value: port,
            })?;
        }
        if let Ok(dir) = std::env::var("NIPA_DATA_DIR") {
            self.server.data_dir = PathBuf::from(dir);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_port_is_11810() {
        assert_eq!(ServerConfig::default().server.port, 11810);
    }

    #[test]
    fn parses_partial_toml() {
        let cfg = ServerConfig::from_toml_str(
            "[server]\nport = 8080\n",
            std::path::Path::new("test.toml"),
        )
        .unwrap();
        assert_eq!(cfg.server.port, 8080);
        // 未给出的字段落默认值
        assert_eq!(cfg.server.bind, "0.0.0.0");
        assert_eq!(cfg.log.filter, "info");
    }
}
