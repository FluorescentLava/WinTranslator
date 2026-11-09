# 第三方声明

应用源码采用 MIT 许可证。第三方代码、数据、运行库和服务标识遵循各自的许可与权利声明。

| 组件 | 来源与许可证 | 许可文件及说明 |
| --- | --- | --- |
| windows-reactor / windows-rs | Microsoft；MIT OR Apache-2.0 | `vendor/windows-reactor` 保留上游许可证，包含字号、颜色与透明背景下光标绘制等本地扩展。 |
| Windows App Runtime | Microsoft | 安装包使用系统注册的 2.4 或更新的兼容 2.x 版本，缺少时下载安装微软官方运行库。许可及声明见 `licenses/windows-app-sdk-LICENSE.txt`、`licenses/windows-app-sdk-NOTICE.txt`；自包含构建分发的运行库遵循相同条款。 |
| Microsoft.Web.WebView2.Core | Microsoft | 自包含构建可能附带此依赖；许可及声明见 `licenses/webview2-LICENSE.txt`、`licenses/webview2-NOTICE.txt`。应用界面使用原生 WinUI 控件。 |
| Microsoft Visual C++ Runtime | Microsoft | 随应用分发的 DLL 来自 Visual Studio Build Tools 的 `VC/Redist/MSVC/x64/Microsoft.VC143.CRT`，遵循 Visual Studio 可再分发组件条款。 |
| Inno Setup 简体中文翻译 | Zhenghan Yang 维护；Inno Setup 许可证 | `installer/Languages/ChineseSimplified.isl` 来自官方 `is-6_7_3` 发行源码；许可证见 `licenses/innosetup-LICENSE.txt`。Inno Setup 用于构建安装程序。 |
| ONNX Runtime 1.24.4 | Microsoft；MIT | 见 `runtime/LICENSE.txt`、`licenses/onnxruntime-ThirdPartyNotices.txt`。 |
| OPUS-MT Marian 翻译模型 | Helsinki-NLP / OPUS，Xenova 转换为 ONNX；Apache-2.0 | 许可依据上游模型卡，见 `licenses/Apache-2.0.txt`；模型清单记录仓库修订与文件完整性信息。 |
| Marian SentencePiece tokenizer 构建逻辑 | 改编自 Hugging Face Candle 的 `candle-examples/examples/marian-mt/spm.rs`；MIT | 见 `licenses/candle-MIT.txt`。 |
| ECDICT | skywind3000 及贡献者；MIT | 见 `dictionary/ECDICT-LICENSE.txt`、`dictionary/SOURCES.md`；词典 CSV 不纳入源码管理。 |
| ort | MIT OR Apache-2.0 | 依赖版本记录于 `Cargo.lock`。 |
| tokenizers | Apache-2.0 | 依赖版本记录于 `Cargo.lock`。 |
| reqwest、serde、serde_json、arboard | MIT OR Apache-2.0 | 其他 Rust 传递依赖的版本记录于 `Cargo.lock`，遵循各自许可证。 |

语言交换箭头由本项目绘制，采用本仓库的 MIT 许可证。应用图标和服务图标的来源见 `assets/icons/sources.md`、`assets/providers/sources.md`。

在线服务的商标及原始图像权利属于各服务所有者。本应用是独立客户端，与百度、腾讯、有道、DeepL、MyMemory 或 Microsoft 没有隶属关系。
