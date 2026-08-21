// 错误分类学是三个前端共享的单一错误模型：一个带 Kind 的错误同时驱动
// CLI 退出码、HTTP 状态码与 MCP JSON-RPC 错误码，传输实现永远不需要去
// 解释命令专属的错误字符串。
//
// 零第三方依赖：CodedError 手写 std::error::Error 实现（不引 thiserror）。

use std::fmt;

/// Kind 给错误分类。前端通过 http_status、exit_code、jsonrpc_code 把
/// Kind 翻译成各自渠道的错误表示。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// 调用方提供了畸形或非法的入参（缺必填、校验失败、枚举越界）。
    InvalidInput,
    /// 调用方必须先行认证。
    Unauthorized,
    /// 调用方已认证但缺少权限。
    Forbidden,
    /// 操作目标不存在。
    NotFound,
    /// 操作与现有状态冲突。
    Conflict,
    /// 操作被调用方取消。
    Canceled,
    /// 依赖的服务暂时不可用。
    Unavailable,
    /// 未分类失败的兜底分类。
    Internal,
}

impl Kind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Kind::InvalidInput => "invalid_input",
            Kind::Unauthorized => "unauthorized",
            Kind::Forbidden => "forbidden",
            Kind::NotFound => "not_found",
            Kind::Conflict => "conflict",
            Kind::Canceled => "canceled",
            Kind::Unavailable => "unavailable",
            Kind::Internal => "internal",
        }
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error 是库内统一的错误类型（CodedError 的 Rust 对应物）。
/// kind 缺省为 Internal；用户在自己的 handler 里也可以直接返回任何
/// `std::error::Error`（由 classify 兜底分类为 Internal）。
#[derive(Debug)]
pub struct Error {
    kind: Kind,
    message: String,
    cause: Option<Box<dyn std::error::Error + Send + Sync + 'static>>,
}

impl Error {
    /// 新建一条没有 cause 的分类错误。
    pub fn new(kind: Kind, msg: impl Into<String>) -> Self {
        Error {
            kind,
            message: msg.into(),
            cause: None,
        }
    }

    /// 用格式串新建一条分类错误。
    pub fn errorf(kind: Kind, format: std::fmt::Arguments<'_>) -> Self {
        Error {
            kind,
            message: fmt::format(format),
            cause: None,
        }
    }

    /// 把任意错误包成分类错误：消息取 cause 的消息。
    pub fn wrap(kind: Kind, cause: impl std::error::Error + Send + Sync + 'static) -> Self {
        Error {
            kind,
            message: String::new(),
            cause: Some(Box::new(cause)),
        }
    }

    /// 既有消息又有 cause 的分类错误。
    pub fn wrap_msg(
        kind: Kind,
        cause: impl std::error::Error + Send + Sync + 'static,
        msg: impl Into<String>,
    ) -> Self {
        Error {
            kind,
            message: msg.into(),
            cause: Some(Box::new(cause)),
        }
    }

    /// 把任意用户错误升级为库错误：只包一层（返回 Box 进 source 链），
    /// classify 会沿链找用户内层已有的分类。用户错误本身不丢。
    pub fn upgrade(cause: impl std::error::Error + Send + Sync + 'static) -> Self {
        Error {
            kind: Kind::Internal,
            message: String::new(),
            cause: Some(Box::new(cause)),
        }
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// 解开一层到最内层已知 cause（Go 的 Cause）。
    pub fn cause(&self) -> Option<&(dyn std::error::Error + Send + Sync)> {
        self.cause.as_deref()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.message.is_empty(), &self.cause) {
            (false, Some(cause)) => write!(f, "{}: {}", self.message, cause),
            (false, None) => f.write_str(&self.message),
            (true, Some(cause)) => write!(f, "{}", cause),
            (true, None) => f.write_str(self.kind.as_str()),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.cause
            .as_ref()
            .map(|c| c.as_ref() as &(dyn std::error::Error + 'static))
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::new(Kind::Internal, format!("io: {e}"))
    }
}

/// 沿错误链找到第一条带分类的错误并返回其 Kind：
/// 未分类的非空错误兜底 Internal。入参需 'static 具体错误（或经
/// source() 拿到的 &dyn Error + 'static）。
pub fn classify(err: &(dyn std::error::Error + 'static)) -> Option<Kind> {
    let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(e) = cur {
        if let Some(ce) = e.downcast_ref::<Error>() {
            return Some(ce.kind);
        }
        // 用户错误链里可能嵌套我们自己的分类错误（例如 io::Error 包了 Error）。
        cur = e.source();
    }
    Some(Kind::Internal)
}

/// 把 Kind 翻译成自然的 HTTP 状态码。
pub fn http_status(kind: Kind) -> u16 {
    match kind {
        Kind::InvalidInput => 400,
        Kind::Unauthorized => 401,
        Kind::Forbidden => 403,
        Kind::NotFound => 404,
        Kind::Conflict => 409,
        Kind::Unavailable => 503,
        Kind::Canceled => 499, // 非标准但广为人知；前端可自行覆盖
        Kind::Internal => 500,
    }
}

/// 把 Kind 翻译成 CLI 进程退出码。
pub fn exit_code(kind: Kind) -> i32 {
    if kind == Kind::InvalidInput {
        2 // 与常规 flag 解析失败同码
    } else {
        1
    }
}

/// 把 Kind 翻译成 JSON-RPC 2.0 错误码。-32000..-32099 是 JSON-RPC 与 MCP
/// 保留给应用自定义服务端错误的区间。
pub fn jsonrpc_code(kind: Kind) -> i64 {
    match kind {
        Kind::InvalidInput => -32602, // Invalid params
        Kind::NotFound => -32001,
        Kind::Conflict => -32009,
        Kind::Unauthorized => -32010,
        Kind::Forbidden => -32011,
        Kind::Canceled => -32012,
        _ => -32603, // Internal error
    }
}

/// 命令 handler 的 canonical 返回类型。
pub type Result<T> = std::result::Result<T, Error>;

/// 便捷构造函数（对齐 Go 的 errs.New/errs.Errorf/errs.Wrap）。
pub fn new(kind: Kind, msg: impl Into<String>) -> Error {
    Error::new(kind, msg)
}

/// 返回带格式化消息的分类错误：
/// `errs::errorf!(errs::Kind::NotFound, "user {} not found", name)`
#[macro_export]
macro_rules! errorf {
    ($kind:expr, $($arg:tt)*) => {
        $crate::errors::Error::errorf($kind, format_args!($($arg)*))
    };
}

pub use errorf;
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mappings() {
        assert_eq!(http_status(Kind::InvalidInput), 400);
        assert_eq!(http_status(Kind::Unauthorized), 401);
        assert_eq!(http_status(Kind::Forbidden), 403);
        assert_eq!(http_status(Kind::NotFound), 404);
        assert_eq!(http_status(Kind::Conflict), 409);
        assert_eq!(http_status(Kind::Unavailable), 503);
        assert_eq!(http_status(Kind::Canceled), 499);
        assert_eq!(http_status(Kind::Internal), 500);

        assert_eq!(exit_code(Kind::InvalidInput), 2);
        assert_eq!(exit_code(Kind::NotFound), 1);

        assert_eq!(jsonrpc_code(Kind::InvalidInput), -32602);
        assert_eq!(jsonrpc_code(Kind::NotFound), -32001);
        assert_eq!(jsonrpc_code(Kind::Conflict), -32009);
        assert_eq!(jsonrpc_code(Kind::Unauthorized), -32010);
        assert_eq!(jsonrpc_code(Kind::Forbidden), -32011);
        assert_eq!(jsonrpc_code(Kind::Canceled), -32012);
        assert_eq!(jsonrpc_code(Kind::Internal), -32603);
    }

    #[test]
    fn classify_chain() {
        let e = Error::wrap_msg(Kind::NotFound, Error::new(Kind::Conflict, "inner"), "outer");
        assert_eq!(classify(&e), Some(Kind::NotFound));
        // 内层 cause 类型也保留在链上（Cause 语义；Error::source() 在
        // std::error::Error 上，给出 &dyn Error + 'static）。
        let cause = std::error::Error::source(&e).unwrap();
        assert_eq!(classify(cause), Some(Kind::Conflict));
    }

    #[test]
    fn error_display_forms() {
        assert_eq!(Error::new(Kind::NotFound, "x").to_string(), "x");
        let wrapped = Error::wrap(Kind::InvalidInput, std::io::Error::other("ioerr"));
        assert_eq!(wrapped.to_string(), "ioerr");
        let both = Error::wrap_msg(Kind::Internal, std::io::Error::other("ioerr"), "boom");
        assert_eq!(both.to_string(), "boom: ioerr");
    }
}
