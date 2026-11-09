# 离线词典数据来源

`ecdict.csv` 是 ECDICT 英汉词典的常用词条子集，供本机查询音标、词性及中英文义项。词典独立于翻译模型和在线翻译服务。

- 上游仓库：[skywind3000/ECDICT](https://github.com/skywind3000/ECDICT)。
- 固定修订：`39cfb9672c0906b63569e0453c2e6abfb9ab182a`。
- [原始 CSV](https://raw.githubusercontent.com/skywind3000/ECDICT/39cfb9672c0906b63569e0453c2e6abfb9ab182a/ecdict.csv)、[字段说明](https://github.com/skywind3000/ECDICT/blob/39cfb9672c0906b63569e0453c2e6abfb9ab182a/README.md#数据格式)。
- 许可证：[上游 MIT LICENSE](https://github.com/skywind3000/ECDICT/blob/39cfb9672c0906b63569e0453c2e6abfb9ab182a/LICENSE)，副本为 `ECDICT-LICENSE.txt`。
- 取得日期：2026-11-08。

原始基础 CSV 有 770,611 条记录。应用保留满足以下任一上游元数据条件的词条：考试标签 `tag` 非空、`collins` 星级非零、`oxford=1`、`bnc` 排名为 1–100000，或 `frq` 排名为 1–100000。子集共 59,137 条，保留所有原始字段。

`scripts/download-dictionary.ps1` 从固定修订下载并生成此子集，筛选与完整性信息记录于 `manifest.json`。CSV 不纳入源码管理，随运行文件部署或由脚本准备。

查询使用 `word`（英文词头）、`phonetic`（音标）、`translation`（中文释义）、`definition`（英文释义）和 `pos`（词性）字段。基础 CSV 的 `pos` 列为空，释义中的词性标注仍可用于显示，例如 `a.`、`s.`、`n.`、`vi.`、`vt.`、`adv.`。显示时将 `a.`、WordNet `s.` 统一为 `adj.`，将 `r.` 显示为 `adv.`；无标注的义项保留空词性。

子集以常用英语词头为主；未收录的词或缺少数据时返回空结果。释义和例句以词典原有内容为限。
