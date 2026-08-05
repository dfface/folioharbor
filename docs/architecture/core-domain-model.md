# 核心领域模型

状态：经方向确认的设计草案

更新时间：2026-08-05

## 建模原则

1. 书目事实、书库馆藏、逻辑副本、物理字节和访问权限必须分离。
2. 核心书目语义采用 IFLA LRM 的 WEMI 层次，不用文件格式替代书目实体。
3. 相同文件允许物理复用，但不能因此共享权限、来源或删除生命周期。
4. 网文、漫画和有声书使用通用内容单元扩展，不把整个系统绑定在“章节”概念上。
5. 外部协议映射不反向污染内部事务模型；任何偏离标准之处必须有书面说明。

## 书目核心

```text
Work
└── Expression
    └── ManifestationExpression ── Manifestation
```

- `Work`：抽象作品，例如某部小说或漫画作品。
- `Expression`：作品的具体表达，例如翻译、修订、删节、朗读或着色版本。
- `Manifestation`：表达被出版或分发的具体形态，包含出版者、日期、格式和版本特征。
- `ManifestationExpression`：Manifestation 与 Expression 的关联，用于选集、多语合刊等多对多场景。

EPUB、PDF 或有声包如果技术与发行特征不同，通常属于不同 Manifestation，而不是同一个实体上的多个随意附件。

## 书库、馆藏和副本

```text
Library
└── Holding ─────────────> Manifestation
    └── Item
        └── ItemAsset
            └── Blob
                └── BlobLocation
```

- `Library`：协作、数据隔离和授权边界。
- `Holding`：某书库对一个 Manifestation 的馆藏记录。
- `Item`：该馆藏下实际持有的逻辑电子副本，保存来源、导入者和生命周期。
- `ItemAsset`：副本所包含的原文件、封面或补充文件。
- `Blob`：以内容哈希标识的不可变字节对象。
- `BlobLocation`：Blob 在本地文件系统或对象存储中的实际位置。

两个书库上传相同文件时，可以拥有两个不同的 Item，同时引用同一个 Blob。权限检查从 Library、Holding 和 Item 出发，不能从 Blob 推导。

## 连载与复合内容扩展

```text
Expression
└── ContentUnit
    └── ContentRevision

Manifestation
└── ManifestationUnit
    └── ContentUnit + ContentRevision + Locator
```

- `ContentUnit`：卷、章、话、页、音轨等有序且可嵌套的逻辑单元。
- `ContentRevision`：连载内容或原生内容的版本历史。
- `ManifestationUnit`：某个 Manifestation 采用哪个内容修订、顺序以及格式相关定位信息。

普通 EPUB 的正文仍以完整原包作为事实源；ContentUnit 和 Locator 用于目录、搜索、进度与批注，不意味着必须把每章永久拆成独立 Blob。

## 阅读数据

- `ReadingState` 属于用户，以 Manifestation、稳定内容单元和 Readium Locator 表达位置。
- `Annotation` 属于用户，采用 W3C Web Annotation 的 Body、Target、Selector 和 Motivation 语义。
- 书库移除 Holding 后，不应立刻硬删除用户的进度和批注；先进入不可访问或可重新关联状态。
- 跨 Manifestation 迁移阅读位置不能只比较百分比，应优先通过 ContentUnit 身份和文本选择器匹配。

## 权限与隔离

- PostgreSQL RLS 提供书库级粗粒度隔离。
- 应用授权引擎统一判断动作，例如 `holding.view`、`item.read`、`item.download` 和 `resource.manage_acl`。
- 资源授权记录表达用户、角色或群组对具体资源的额外许可。
- 系统管理员不自动拥有内容读取权；紧急访问必须提供原因、限时并进入不可变审计记录。

## 关键不变量

- Blob 不拥有书目语义或访问权限。
- 一个 Item 只能位于一个 Holding 下。
- 一个 Holding 只能属于一个 Library，并指向一个 Manifestation。
- 删除某个 Item 不得删除仍被其他 Item 引用的 Blob。
- 全局书目实体不能因缺少可访问 Holding 而被普通用户枚举。
- `public` 是资源可见性；是否发布到联邦是独立状态。
- 公开元数据、正文阅读、原文件下载和联邦发布必须分别授权，不能相互推导。
