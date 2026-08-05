# FolioHarbor 标准应用规范草案

状态：讨论草案

暂定版本：0.1

## 目的

FolioHarbor 不发明另一套封闭的电子书协议。本应用规范定义内部模型如何采用或映射现有标准，以及哪些能力属于书港扩展。

标准用于稳定外部语义和交换边界，不要求 PostgreSQL 直接保存 RDF 图，也不要求第一版实现完整的图书馆编目系统。

## 规范矩阵

| 领域 | 基准标准 | FolioHarbor 用途 |
| --- | --- | --- |
| 书目概念 | IFLA Library Reference Model | Work、Expression、Manifestation、Item 的语义和关系 |
| 图书馆数据交换 | BIBFRAME 2.0 | Work、Instance、Item、Agent 和 Contribution 的映射参考 |
| EPUB | W3C EPUB 3.3 | 包结构、导航、资源和媒体类型 |
| 阅读清单 | Readium Web Publication Manifest | Web 与未来移动客户端之间的出版物描述 |
| 阅读位置 | Readium Locator | 进度、书签、搜索结果和批注位置 |
| 目录分发 | OPDS 2.0 | 公开或授权书目目录与获取链接 |
| 批注 | W3C Web Annotation Data Model | Body、Target、Selector、Motivation 与可移植批注 |
| 身份认证 | OpenID Connect Core | 本地账户之外的 SSO 身份认证 |
| 联邦 | ActivityPub / ActivityStreams 2.0 | 关注、书评、书单、推荐和撤回活动 |
| 语言 | BCP 47 | 元数据、Expression 和内容语言标签 |
| 内容类型 | IANA Media Types | Blob、资源接口、Locator 和下载响应 |
| HTTP API | OpenAPI 3.1 | API 契约、客户端生成和兼容性测试 |

## 内部模型映射

| FolioHarbor | IFLA LRM | BIBFRAME | 说明 |
| --- | --- | --- | --- |
| `Work` | Work | Work | 抽象作品 |
| `Expression` | Expression | 通常并入 Work 或通过关系表达 | 翻译、修订、演绎等表达差异 |
| `Manifestation` | Manifestation | Instance | 具体出版或技术形态 |
| `Item` | Item | Item | 某个书库实际管理的电子副本 |
| `Holding` | 非独立核心实体 | Item/holding 相关关系 | 书库对 Manifestation 的馆藏关系 |
| `Blob` | 无对应实体 | 电子定位的一部分 | 书港的存储实现扩展 |
| `ContentUnit` | 聚合或组成关系 | 部分/关系 | 网文、漫画和有声内容结构扩展 |

## 标识符规则

- 所有内部实体使用不可变 UUID，不从标题、文件名或 ISBN 派生主键。
- 外部标识符使用独立关联记录，至少保存 `scheme`、`value`、`issuer`、`normalized_value` 和来源。
- ISBN 通常标识 Manifestation，不能直接作为 Work 的全局唯一标识。
- 远程联邦对象使用可解析 URI；本地 UUID 与远程 URI 不互相替代。
- 语言使用 BCP 47，文件格式使用注册的媒体类型。

## 扩展规则

- ActivityStreams 扩展使用书港自己的版本化 JSON-LD 命名空间，不能覆盖标准词义。
- 本地扩展字段必须能被未知扩展的客户端安全忽略。
- OPDS、批注和联邦对象应提供 JSON Schema 或相应的契约测试样例。
- 任何无法无损映射的字段必须记录来源和原始值，避免导入时悄悄丢失信息。

## 一致性验证方向

- 用公开 EPUB 样本验证解析与原包往返不修改源文件。
- 对 OPDS、Readium Locator、Web Annotation 和 ActivityStreams 建立固定 JSON 样例测试。
- 验证导出后重新导入不会丢失核心书目语义、标识符和来源。
- 对不支持的扩展执行保留或明确拒绝，不能静默解释为其他字段。

## 参考规范

- [IFLA Library Reference Model](https://repository.ifla.org/items/214c74cb-c075-4428-a138-39f8d06c55aa)
- [BIBFRAME 2.0 Model](https://www.loc.gov/bibframe/docs/bibframe2-model.html)
- [EPUB 3.3](https://www.w3.org/TR/epub-33/)
- [Readium Locators](https://readium.org/architecture/models/locators/)
- [OPDS 2.0](https://specs.opds.io/opds-2.0)
- [Web Annotation Data Model](https://www.w3.org/TR/annotation-model/)
- [OpenID Connect Core 1.0](https://openid.net/specs/openid-connect-core_1_0-18.html)
- [ActivityPub](https://www.w3.org/TR/activitypub/)
- [ActivityStreams 2.0](https://www.w3.org/TR/activitystreams-core/)
- [BCP 47 / RFC 5646](https://datatracker.ietf.org/doc/html/rfc5646)
