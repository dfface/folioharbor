# FolioHarbor · 书港

FolioHarbor（中文名：书港）是一个面向个人、家庭和小型团队的开源私人数字图书馆。

项目当前处于首个 EPUB 端到端切片的实现阶段。需求、决策和标准化设计仍然是实现的约束来源。

## 当前方向

- 多用户、自托管，支持私人书库、协作书库和细粒度数据权限。
- 支持本地账户以及 OIDC SSO。
- 首批支持 EPUB、TXT、PDF 的上传、在线阅读和原文件下载。
- 原文件是不可变事实源，支持本地磁盘和 S3 兼容对象存储。
- PostgreSQL 保存业务数据，Meilisearch 作为可选、可重建的全文索引。
- 采用标准化书目模型，并为未来 Android 客户端、OPDS 和联邦社区预留边界。
- 旧 `ebooks-go` 仅作为只读的数据治理与迁移来源，不在本仓库中延续旧实现。

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
