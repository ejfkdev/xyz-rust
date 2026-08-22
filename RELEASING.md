# Releasing

一个版本 = 一个带完整变更清单的 annotated tag。步骤：

1. `git tag -n1 <上个版本>` 复核上版本的 message 格式；
2. 确定版本号（Go v0 模块：破坏 → 升 MINOR、其余 → PATCH；Rust 遵循
   semver；spec 遵循 §17.1 MAJOR/MINOR/PATCH 规则）；
3. 打 tag：**message 必须包含** 一句话主题 + 自上一版本起的分条变更
   （每条一行、可分类，含关联的 spec 章节号/回归测试名），形如：

```bash
git tag -a v0.3.1 -m "xyz-go v0.3.1 — <主题>

自 v0.3.0 以来的变更：

- <功能/修复一>
- <功能/修复二>"
git push origin v0.3.1
```

4. 推送后立即 `GOPROXY=direct go list -m …@<版本>`（Go）或
   `git ls-remote --tags origin`（其余）验证可解析；
5. 同步更新 CONFORMANCE 目标（spec 版本）与本仓库 README 的版本表。

The annotated tag message *is* the release note — a bare version number
is not a release.
