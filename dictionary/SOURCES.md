# 离线词典数据来源

此目录中的 `ecdict.csv` 是 ECDICT 英汉词典的常用词条子集，供本机离线查询音标、真实词性标注及中英文多义项。词典独立于翻译模型和在线翻译服务，未从机器译文推测词性。

- 上游仓库：<https://github.com/skywind3000/ECDICT>
- 固定修订：`39cfb9672c0906b63569e0453c2e6abfb9ab182a`
- 原始数据：<https://raw.githubusercontent.com/skywind3000/ECDICT/39cfb9672c0906b63569e0453c2e6abfb9ab182a/ecdict.csv>
- 许可证：上游该修订的 [MIT LICENSE](https://github.com/skywind3000/ECDICT/blob/39cfb9672c0906b63569e0453c2e6abfb9ab182a/LICENSE)，未经修改的副本为 `ECDICT-LICENSE.txt`。
- 上游字段说明：<https://github.com/skywind3000/ECDICT/blob/39cfb9672c0906b63569e0453c2e6abfb9ab182a/README.md#数据格式>
- 取得日期：2026-11-08。

原始基础 CSV 共 770,611 条，66,009,548 字节，SHA-256 为 `d1ecbdf9ee2b920afb6f430ca6b6bb9f559588cf0dff86e04bd255f9390843cc`。为了缩短启动时建立查询索引的时间，保留满足以下任一上游元数据条件的条目：考试标签 `tag` 非空、`collins` 星级非零、`oxford=1`、`bnc` 排名在 1–100000，或 `frq` 排名在 1–100000。

筛选结果为 59,137 条、10,437,264 字节；当前 UTF-8/LF CSV 的 SHA-256 为 `66725c8bb2519410dbf7878c6b3cf5998885eced18ee946dd285f106f22990c2`。筛选未指定任何测试单词，所有原始字段均保留。`scripts/download-dictionary.ps1` 可从固定修订重新下载、验证源文件并生成同样的词条集合；文件校验及筛选记录见 `manifest.json`。CSV 数据不应提交到 Git，应随运行文件部署或由脚本重新下载。

使用的字段为 `word`（英文词头）、`phonetic`（上游音标）、`translation`（中文释义）、`definition`（英文释义）、`pos`（上游专用词性字段）。基础 CSV 的 `pos` 列为空，但中英文释义字段自身保留了真实词性标注，例如 `a.`、`s.`、`n.`、`vi.`、`vt.`、`adv.`；模块仅对这些原始标注做常规显示映射，如 `a.` / WordNet `s.` 显示为 `adj.`，`r.` 显示为 `adv.`，不按词义猜测词性。

无标注的义项保留空词性；未收录的词或未下载数据时查询返回空结果。此子集以常用英语词头为主，不声称覆盖全部罕见词、专名、短语或中文词头，也不生成例句或补写词典未提供的信息。
