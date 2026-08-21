// Ctx 是贯穿三个前端的取消信号：Go 侧 context.Context 的 Rust 对应物。
// 零依赖（Arc<AtomicBool>）——核心不为取消机制引入任何运行时依赖，
// 各前端用自己的方式接线（tokio signal / ctrlc）。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// 可克隆的取消上下文。handler 用 `ctx.cancelled()` 做周期检查，
/// 长任务可在退出前排空。
#[derive(Clone, Debug, Default)]
pub struct Ctx {
    cancelled: Arc<AtomicBool>,
}

impl Ctx {
    /// 新建一个未取消的上下文。
    pub fn new() -> Self {
        Ctx::default()
    }

    /// 报告是否已收到取消（SIGINT/SIGTERM 或显式 cancel）。
    pub fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    /// 发出取消信号（幂等；由信号接线与测试调用）。
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    /// 返回一个可以在线程/任务间共享的底层句柄（信号线程用）。
    pub fn raw(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancelled)
    }
}

#[cfg(feature = "http-stack")]
impl Ctx {
    /// 在 tokio 任务里等待取消（HTTP/MCP 模式的优雅关停 select 用）。
    pub async fn cancelled_async(&self) {
        let raw = self.raw();
        tokio::task::yield_now().await;
        while !raw.load(Ordering::Relaxed) {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }
}
