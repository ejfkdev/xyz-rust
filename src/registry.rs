// registry 持有 spec 构建的条目并把它们交给前端。注册时急切校验名字
// 唯一，冲突在启动期浮出而不是首次调用时。
//
// 本模块同时拥有进程级默认注册表（default()）：one-main 程序用
// define(...) 注册进它、让 dispatch 派发它，用户代码无需构建或引入
// 注册表。

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use crate::errors;
use crate::spec::Entry;

/// Registry 是所有前端共享的中央命令表。
pub struct Registry {
    entries: RwLock<HashMap<String, Arc<Entry>>>,
}

impl Default for Registry {
    fn default() -> Self {
        Registry {
            entries: RwLock::new(HashMap::new()),
        }
    }
}

impl Registry {
    /// 空注册表。
    pub fn new() -> Self {
        Registry {
            entries: RwLock::new(HashMap::new()),
        }
    }

    /// 进程级默认注册表。
    #[allow(clippy::should_implement_trait)]
    pub fn default() -> &'static Registry {
        static DEFAULT: OnceLock<Registry> = OnceLock::new();
        DEFAULT.get_or_init(Registry::new)
    }

    /// 注册一条条目。名字冲突是错误。
    pub fn add(&self, e: Arc<Entry>) -> errors::Result<Arc<Entry>> {
        if e.name.is_empty() {
            return Err(errors::Error::new(
                errors::Kind::Internal,
                "registry: entry has empty name".to_string(),
            ));
        }
        let mut entries = self.entries.write().unwrap();
        if let Some(old) = entries.get(&e.name) {
            return Err(errors::Error::new(
                errors::Kind::Internal,
                format!(
                    "registry: name {:?} already registered (existing summary {:?})",
                    e.name, old.summary
                ),
            ));
        }
        entries.insert(e.name.clone(), Arc::clone(&e));
        Ok(e)
    }

    /// 按名取条目。
    pub fn get(&self, name: &str) -> Option<Arc<Entry>> {
        self.entries.read().unwrap().get(name).cloned()
    }

    /// 全部注册名，排序。
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.entries.read().unwrap().keys().cloned().collect();
        names.sort();
        names
    }

    /// 全部条目，按名排序。
    pub fn all(&self) -> Vec<Arc<Entry>> {
        let mut out: Vec<Arc<Entry>> = self.entries.read().unwrap().values().cloned().collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::Ctx;
    use crate::spec::command::Command;

    fn entry(name: &str) -> Arc<Entry> {
        #[derive(crate::XyzArgs)]
        struct A {
            #[xyz(desc = "s")]
            s: String,
        }
        fn h(_: &Ctx, _: &A) -> crate::errors::Result<String> {
            Ok(String::new())
        }
        Arc::new(Command::new(name, h).entry().unwrap())
    }

    #[test]
    fn add_dup_is_error() {
        let r = Registry::new();
        let e = entry("a.b");
        r.add(Arc::clone(&e)).unwrap();
        let err = r.add(Arc::clone(&e)).unwrap_err();
        assert!(err.to_string().contains("already registered"), "{err}");
    }

    #[test]
    fn names_and_all_sorted() {
        let r = Registry::new();
        for n in ["c.x", "a.y", "b.z"] {
            r.add(entry(n)).unwrap();
        }
        assert_eq!(r.names(), vec!["a.y", "b.z", "c.x"]);
        let all = r.all();
        assert_eq!(
            all.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            vec!["a.y", "b.z", "c.x"]
        );
    }

    #[test]
    fn get_hit_and_miss() {
        let r = Registry::new();
        r.add(entry("x.y")).unwrap();
        assert!(r.get("x.y").is_some());
        assert!(r.get("nope").is_none());
    }
}
