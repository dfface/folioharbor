# 设计文档索引

本目录是 FolioHarbor 的长期事实来源。对话负责探索，文档负责记录已经形成的共识、依据和仍待解决的问题。

## 文档分区

| 目录 | 内容 | 当前状态 |
| --- | --- | --- |
| `requirements/` | 产品愿景、用户、场景、范围和验收标准 | 草案 |
| `architecture/` | 系统边界、核心领域模型和数据流 | 草案 |
| `standards/` | 书目、阅读、认证、目录、批注和联邦标准映射 | 草案 |
| `decisions/` | 已作出的架构决策记录（ADR） | 持续维护 |
| `research/` | 旧系统、真实数据、协议和竞品调研 | 持续维护 |
| `migration/` | 旧数据治理、去重、验证和迁移方案 | 尚未展开 |
| `superpowers/specs/` | 经过逐段确认和审阅的正式设计规格 | 首份规格等待用户复核 |

## 状态规则

- **调研记录**可以包含事实、推断和风险，但必须标明证据与置信度。
- **需求草案**可以保留开放问题，不能被当作实现承诺。
- **ADR**记录已经确认的方向；如有变化，以新的 ADR 取代，保留历史原因。
- **正式规格**只有在逐段获得确认、自审通过并由用户复核后才能进入 `superpowers/specs/`。
- **实施计划**必须以正式规格为输入，不能直接从聊天记录生成。

## 当前阅读顺序

1. [第一条 EPUB 端到端纵切设计](superpowers/specs/2026-08-05-first-epub-vertical-slice-design.md)
2. [全量 Brainstorm 纪要](research/2026-08-04-to-2026-08-05-full-brainstorm.md)
3. [产品愿景与范围](requirements/product-vision.md)
4. [核心领域模型](architecture/core-domain-model.md)
5. [标准应用规范草案](standards/application-profile.md)
6. [旧系统数据调研摘要](research/legacy-ebooks-audit.md)
7. `decisions/` 中的架构决策记录
