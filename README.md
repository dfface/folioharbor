# FolioHarbor · 书港

FolioHarbor（中文名：书港）是一个面向个人、家庭和小型团队的开源私人数字图书馆。

项目当前完成了首个 EPUB 端到端切片的候选实现。需求、决策和标准化设计仍然是实现的约束来源；发布前必须通过 [发布检查表](docs/operations/release-checklist.md)。

## 当前已实现的切片

- 多用户、自托管，本地账户验证，私人书库、协作书库、邀请和细粒度角色权限。
- EPUB 上传、后台校验与编目、在线资源读取，以及单独授权的原文件 Range 下载。
- PostgreSQL 18 保存业务状态并强制 RLS；本地磁盘保存不可变 Blob，并支持配额、共享 Blob 生命周期和 GC 恢复。
- 跨设备阅读进度同步，以及 API、Worker、CLI、Web 和 Compose 的运维健康边界。
- 旧 `ebooks-go` 仅作为只读的数据治理与迁移来源，不在本仓库中延续旧实现。

OIDC、TXT/PDF 阅读、OPDS、批注、S3 兼容对象存储、搜索、联邦和移动客户端仍是未来工作，不属于本次发布能力。

## 文档入口

请从 [设计文档索引](docs/README.md) 开始阅读。

## Rust 工作区

工作区固定使用 Rust 1.88.0。提交前运行：

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo deny check
```

完整发布验证（包括 Web、PostgreSQL 18、Playwright 和容器启动）见 [首个切片发布检查表](docs/operations/release-checklist.md)。
