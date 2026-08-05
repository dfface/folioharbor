# ADR-0002：采用 WEMI 作为书目核心

状态：已接受

日期：2026-08-05

## 背景

`Book`、`Edition`、`Publication` 和 `Rendition` 在日常语言中容易混用，无法稳定表达翻译、修订、出版形态、文件格式和实际副本之间的关系。

## 决策

核心书目模型使用 IFLA LRM 的：

```text
Work → Expression → Manifestation → Item
```

书港额外定义 Holding、Blob、ContentUnit 和 ContentRevision，但不把这些扩展冒充为标准书目实体。

## 结果

- 翻译和演绎有明确的 Expression 层。
- EPUB、PDF、连载和有声形态可以在 Manifestation 层区分。
- Item 能表达某个书库实际持有的电子副本。
- 可建立到 BIBFRAME、OPDS 和联邦对象的可解释映射。
