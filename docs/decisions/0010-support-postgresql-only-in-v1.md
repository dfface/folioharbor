# ADR-0010：第一版只正式支持 PostgreSQL

状态：已接受

日期：2026-08-05

## 背景

SQLx 可以连接 PostgreSQL、MySQL 和 SQLite，但连接驱动兼容不等于业务语义兼容。FolioHarbor 已确认使用 PostgreSQL RLS 作为书库隔离防线，并依赖持久任务、并发 Worker、迁移锁和结构化元数据。要求多个数据库完全等价会产生多套 SQL、migration、锁、权限和测试实现，并迫使核心设计采用最低能力集合。

## 决策

- 第一版唯一正式支持的关系数据库是 PostgreSQL。
- Rust 持久化实现使用 PostgreSQL 专用 SQLx 类型，例如 `PgPool`，不使用 `AnyPool` 伪装运行时可移植性。
- migration 可以使用 PostgreSQL RLS、JSONB、advisory lock、`FOR UPDATE SKIP LOCKED`、部分索引等原生能力。
- 领域模块与持久化实现仍保持明确边界，以控制耦合和便于测试，但第一版不创建 MySQL 或 SQLite 适配器。
- 数据库集成测试必须运行真实 PostgreSQL，不能以 SQLite 内存库代替。
- 文档、部署检查和启动错误必须明确数据库种类与支持版本，不能宣传为数据库无关。
- 未来增加其他数据库需要新的 ADR、独立 migration、能力矩阵和完整契约测试，不能降低 PostgreSQL 模式的安全保证。

## 结果

第一版可以完整利用 PostgreSQL 的租户隔离和并发能力，只维护一套可验证的 schema 与查询。低资源部署优先通过连接数、缓存、Worker 数量和容器配置优化，而不是在核心产品尚未稳定前增加 SQLite Lite 模式。
