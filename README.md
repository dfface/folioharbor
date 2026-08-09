# FolioHarbor · 书港

> [!WARNING]
> ## 项目已终止
>
> FolioHarbor 不再继续开发、发布或维护。该仓库保留为一次产品与技术
> 探索的记录，不应部署到生产环境，也不应依赖其中的安全、兼容性或运
> 维承诺。当前 staging 与现有代码仅供历史参考。
>
> 在终止前完成的联邦阅读网络可行性调研见
> [调研简报](docs/research/2026-08-09-federated-reading-network-feasibility.md)。

FolioHarbor（中文名：书港）是一个面向个人、家庭和小型团队的开源私人数字图书馆。

项目当前完成了首个 EPUB 端到端切片的候选实现。需求、决策和标准化设计仍然是实现的约束来源；发布前必须通过 [发布检查表](docs/operations/release-checklist.md)。

## 当前能力

- 自托管的多用户本地账户认证、个人书库与协作书库、邀请和细粒度角色权限。
- EPUB 上传、后台校验与编目、浏览器内资源阅读，以及单独授权的原文件 Range 下载。
- 跨设备阅读进度同步；刷新或重新打开图书后可恢复进度。
- PostgreSQL 18 保存业务状态并强制 Row-Level Security（RLS）；本地磁盘保存不可变 Blob，并支持配额、共享 Blob 生命周期与 GC 恢复。
- 独立的 API、Worker、运维 CLI 和 React Web 应用，以及用于生产形态部署的 Compose 健康边界。
- API 和 Worker 使用独立的最小权限数据库角色；Compose 将 PostgreSQL 与 Blob 存储保留在内部网络，只暴露回环绑定的 Web 反向代理端口。
- 旧 `ebooks-go` 仅作为只读的数据治理与迁移来源，不在本仓库中延续旧实现。

OIDC、TXT/PDF 阅读、OPDS、批注、S3 兼容对象存储、搜索、联邦和移动客户端仍是未来工作，不属于本次发布能力。

## 架构与目录

```text
apps/       API、Worker 和运维 CLI 进程
crates/     领域/应用层，以及 PostgreSQL、存储、EPUB、HTTP 适配器
web/        React Web 应用与浏览器测试
migrations/ SQLx PostgreSQL schema migrations
openapi/    版本化 HTTP 契约
deploy/     生产形态的 Compose 拓扑与部署配置
tests/e2e/  Playwright 生产形态端到端测试拓扑
scripts/    运维与验证命令，包括 smoke.sh
docs/       产品、设计、运维与发布文档
```

领域与应用逻辑位于 Rust workspace 的内层；PostgreSQL、文件系统、EPUB
解析和 HTTP 是向外适配器。Web 通过版本化 OpenAPI 契约访问 API，Worker
异步处理导入与恢复工作。

## 文档入口

请从 [设计文档索引](docs/README.md) 开始阅读。

## 运行与验证

### Rust 与 Web 质量检查

工作区固定使用 Rust 1.88.0。提交前运行：

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo deny check
```

Web 静态检查与组件测试：

```bash
pnpm --dir web lint
pnpm --dir web typecheck
pnpm --dir web test -- --run
```

### 自动化端到端测试

安装依赖和 Chromium 后运行 Playwright：

```bash
pnpm install --frozen-lockfile
pnpm --dir web exec playwright install chromium
pnpm --dir web exec playwright test
```

该测试会创建并在结束时清理一套独立、生产形态的 Docker Compose 拓扑；不要把它当作常驻 staging 环境。环境组成、失败诊断和安全边界见
[端到端测试说明](tests/e2e/README.md)。

### Staging 人工验收环境

Staging 使用与生产相同的 Compose 拓扑，但有独立且不进入 Git 的配置与密钥：

```bash
cp deploy/staging.env.example deploy/.env.staging
mkdir -m 0700 deploy/secrets.staging
# 按 deploy/.env.staging 创建八个密钥文件后：
chmod 600 deploy/secrets.staging/*
scripts/smoke.sh check
scripts/smoke.sh up --admin-email admin@staging.example
```

`up` 会以交互方式设置管理员密码并等待服务健康；随后运行
`scripts/smoke.sh smoke` 可显示人工 EPUB 验收清单。真实 SMTP、HTTPS
反向代理、八个密钥文件、停止/销毁行为及故障诊断见
[部署与 staging 操作指南](deploy/README.md)。

完整发布验证（包括 Web、PostgreSQL 18、Playwright 和容器启动）见
[首个切片发布检查表](docs/operations/release-checklist.md)。
