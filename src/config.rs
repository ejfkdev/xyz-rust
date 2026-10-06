// 派发配置：模式词、能力开关与内置参数。零值即默认可直接使用；
// 命令行 --xyz.* 与各模式的裸名 flag 优先级高于这里的字段值。

use std::time::Duration;

use crate::logx;

/// ModeWords 重命名内建模式关键词。留空的字段保持默认（serve、mcp、
/// help）。
#[derive(Debug, Clone, Default)]
pub struct ModeWords {
    pub serve: String,
    pub mcp: String,
    pub help: String,
}

/// Capabilities 在运行时开关通道（与 build feature 相互独立）。零值保持
/// 全部可用。禁用通道只移除它自己的运行时路径：模式词（serve/mcp/help）
/// 与 -v/--version 继续工作，被禁用的模式以清晰错误应答。被禁通道的
/// CLI()/HTTP()/MCP() 配置照常编译与运行——只是不再被消费。
#[derive(Debug, Clone, Copy, Default)]
pub struct Capabilities {
    /// 不在命令注册表上生成子命令（mcp/serve/help/-v 仍可用）。
    pub no_cli: bool,
    /// mcp 模式不可用（stdio/http 都拒绝）。
    pub no_mcp: bool,
    /// serve 模式不可用。
    pub no_http: bool,
}

/// Config 调整派发器。零值保持全部默认。
#[derive(Debug, Clone, Default)]
pub struct Config {
    pub modes: ModeWords,
    pub capabilities: Capabilities,

    /// serve 与 mcp(http) 模式的默认监听地址（各模式自己的 --addr flag
    /// 优先）。
    pub addr: String,
    /// 开启 serve REST 与 MCP http 传输的 Bearer 凭据校验，每个元素是
    /// 一个可接受的 token；空表示不校验。命令行写法：--xyz.bearer=tok1,tok2
    /// （stdio 传输为本地进程，不受影响）。
    pub bearer_tokens: Vec<String>,

    /// 库自身诊断的日志级别（logx 输出到 stderr）。零值（LevelUnset）
    /// 保持默认 Info。命令行：--xyz.log-level=debug|info|warn|error。
    pub log_level: crate::logx::Level,
    /// serve 模式的每请求超时（TimeoutLayer）；0 = 不设超时。
    /// （标准头超时在 Rust 实现里未单独配置，见 README 与 Go 版差异节。）
    pub timeout: Duration,
    /// cert_file/key_file 同时给定则 serve 以 TLS 监听
    /// （--xyz.tls-cert/--xyz.tls-key）。
    pub cert_file: String,
    pub key_file: String,
    /// CORSOrigins 非空则开启 CORS：逐个 Origin 放行（"*" 表示任意来源），
    /// OPTIONS 预检在鉴权之前应答。命令行：--xyz.cors=origin1,origin2。
    pub cors_origins: Vec<String>,

    /// 界面语言覆盖：""=自动（--xyz.lang flag > 本字段 > LANG/LC_ALL 环境
    /// 检测 > 英文默认）。取值 "en" | "zh-CN"。
    pub lang: String,
    /// 用户的多语言内容覆盖表：语言 → (消息键 → 文本)。键名见 lang 目录
    /// （xyz-spec §15.8 的规范键表）。
    pub translations: std::collections::HashMap<String, std::collections::HashMap<String, String>>,

    /// serve/mcp 启动时注入的通道级默认参数（字段线上名 → 字符串值）：
    /// 请求/调用未显式提供时自动补上，优先级高于全局 default 属性、低于
    /// 显式入参与接口默认。命令行：--default k=v（可重复/逗号分隔对）。
    pub channel_defaults: std::collections::HashMap<String, String>,

    /// help 总览的自定义文本块：前者原样插在总览开头（程序名/描述/版本/
    /// 仓库地址等自己拼），后者插在结尾（命令表之后，即使命令表被隐藏也
    /// 打印）。空 = 不插入。
    pub help_before: String,
    pub help_after: String,

    /// 应用名（spec §11.6/§12.6）：X-App-Name 响应头与 MCP serverInfo.name。
    /// 空 = 二进制 basename。
    pub name: String,
    /// 应用版本（spec §11.6/§12.6）：X-App-Version 响应头与
    /// serverInfo.version。空 = version::version()（默认 "dev"）。
    /// 与 xyz 库自身的版本（version::SDK_VERSION → X-XYZ-Version）无关。
    pub version: String,
    /// --format 的默认值（--xyz.format 注入）："" = text。
    /// 裸 --format/--json 在未被命令 flag 遮蔽时覆盖之（spec §10.7）。
    pub format: String,
    /// 附加到每个 HTTP 响应的自定义静态头（键值原样写入；与自动
    /// X-App-*/X-XYZ-* 头并存）。命令行：--xyz.header k=v（可重复）。
    pub response_headers: Vec<(String, String)>,
    /// 关闭自动服务器上下文头（HTTP 的 X-App-*/X-XYZ-* 与 MCP 结果的
    /// _meta.xyz）：spec §11.6/§12.8 的单一开关。自定义头不受影响。
    /// 命令行：--xyz.no-server-headers。
    pub no_server_headers: bool,
}

impl Config {
    pub fn default_log_level(&self) -> logx::Level {
        self.log_level
    }

    /// 应用名：config 覆盖 > 二进制 basename（spec §11.6/§12.6）。
    pub fn resolved_name(&self) -> String {
        if !self.name.is_empty() {
            return self.name.clone();
        }
        crate::cli::bin_name()
    }

    /// 应用版本：config 覆盖 > 版本槽（默认 "dev"）。
    pub fn resolved_version(&self) -> String {
        if !self.version.is_empty() {
            return self.version.clone();
        }
        crate::version::version().to_string()
    }
}
