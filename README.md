<p align="center">
  <img src="assets/icons/app.png" width="128" height="128" alt="WinTranslater 图标" />
</p>

# WinTranslater

Windows 11 x64 翻译工具，使用 Rust、原生 WinUI 3 控件与 Mica 背景。支持快捷键唤出、划词翻译、在线服务切换、本地词典和中英离线翻译。

## 下载与安装

在 [GitHub Releases](https://github.com/FluorescentLava/WinTranslator/releases) 下载并运行 `WinTranslater-0.1.0-Beta-windows-x64-setup.exe`。安装包包含本地 ECDICT 词典、ONNX Runtime 和应用所需的 Visual C++ DLL，无需安装 Rust 或 Python。中英双向离线模型在应用内按需下载，安装应用无需先下载模型。API 凭据在设置中填写。

默认安装到 `%LOCALAPPDATA%\Programs\WinTranslater`，无需管理员权限，提供开始菜单入口和系统卸载项。程序需要为当前用户注册的 Windows App Runtime 2.4 或更新的兼容 2.x x64 版本；缺少时，安装程序从微软下载官方安装程序并安装。已有兼容运行库时可离线安装应用。

从 Windows“已安装的应用”卸载，或运行安装目录内的 `unins000.exe`。升级和卸载保留本机配置、凭据、历史、用量记录及用户目录中的离线模型；系统共享运行库由 Windows 维护。

## 使用

- 按 `Ctrl+Tab` 在当前虚拟桌面唤出窗口，失焦后自动收起。关闭窗口后程序继续驻留，通过托盘菜单退出；可在设置中管理开机自启。
- 划选文字后按 `Ctrl+Tab`，可填入选区并自动翻译。在翻译窗口内划选时直接翻译，保持当前窗口与焦点。划词使用 Windows UI Automation，不改动剪贴板；应用未提供文本选区时，窗口保留原输入。
- 输入后停顿 350ms 自动翻译，继续输入可取消未完成的任务。中英语言对按输入识别方向；手动选择其他语言后保留所选语言对。
- 点击结果卡片展开或收起词典释义，点击发音按钮播放英语读音。英、美音标分别来自对应口音数据；短文本优先使用在线词典音频，缓存可离线使用，音频不可用或文本较长时使用本机对应口音语音。
- 底部服务按钮选择首选服务，结果标识显示实际提供译文的服务。设置窗口可管理 API 凭据、免费用量估算和自动离线；历史页支持搜索记录。

## 在线服务与免费额度

默认首选无需密钥的 MyMemory。程序先尝试所选的可用免费服务，再依次尝试尚未使用的百度、DeepL、腾讯、有道、MyMemory。额度不足、限流、网络或鉴权失败时尝试下一家；全部失败或在线等待预算耗尽后，启用“自动离线”才会调用本地模型。手动选择“本地离线”可直接在设备上翻译。

| 服务或计划 | 所需配置 | 本程序采用的免费上限 |
| --- | --- | --- |
| 百度翻译标准版 | APP ID、密钥 | 每月 50,000 字符 |
| DeepL Developer | API Key，选择 Developer | 总量 1,000,000 字符，不按月重置 |
| DeepL API Free 旧版 | 原有 API Key，选择旧版计划 | 每月 500,000 字符 |
| 腾讯有效免费资源包 | SecretId、SecretKey、已核实的免费资源包 | 配置的有限字符额度，到期停用，默认关闭 |
| 有道有效免费试用 | 应用 ID、密钥、已核实的免费试用额度 | 配置的有限字符额度，到期停用，默认关闭 |
| MyMemory | 无需密钥 | 匿名每天 5,000 字符 |

腾讯和有道仅填写凭据不会启用，须确认免费字符数与到期时间；现金余额或体验金金额不作为免费字数。DeepL 付费计划不加入自动路由。

用量在请求前持久预留，失败和取消也保守计入。设置中的余量是本程序的本地估算，不代表服务账户的实时额度；其他客户端的使用和服务方计费规则仍可能影响账户。百度超出免费额度可能收费，需自行确认账户设置。配置方法、请求限制及账本规则见 [免费额度说明](docs/free-usage.md)。

## 本地词典与离线翻译

本地释义来自 [ECDICT](dictionary/SOURCES.md)，安装完成即可查询。离线翻译使用 ONNX Runtime 和 OPUS-MT 量化模型，支持简体中文与英语互译。在“设置 → 翻译服务”的“离线模型”卡中点击“下载离线模型”，从 Hugging Face 的 [Xenova/opus-mt-en-zh](https://huggingface.co/Xenova/opus-mt-en-zh) 和 [Xenova/opus-mt-zh-en](https://huggingface.co/Xenova/opus-mt-zh-en) 原仓库下载两个方向的模型，合计约 220 MiB。

下载的模型保存在 `%LOCALAPPDATA%\TranslationTool\models`，界面显示“中文 ⇄ 英语 · 已就绪”后即可使用。升级可复用用户目录或旧安装目录中已有的完整模型，无需重复下载。模型清单记录上游修订、文件大小和完整性信息。词典查询和已准备模型的推理不需要联网或 Python；词典释义与机器译文来自不同来源，覆盖范围和翻译准确度各有限制。

模型就绪后，超过 5,000 个字符的文本自动使用本地模型翻译全文，按句界、词界与模型 token 数分段并保留段落。此时手动选择在线服务只翻译前 5,000 个 Unicode 字符，界面会提示未翻译的后续内容；在线失败且启用自动离线时，本地模型翻译全文。

源码运行时，使用以下 PowerShell 脚本准备运行库与词典，模型也可通过应用内按钮下载：

```powershell
.\scripts\download-runtime.ps1
.\scripts\download-dictionary.ps1
```

ONNX Runtime 位于 `runtime/`，词典位于 `dictionary/`。开发时可选用 `.\scripts\download-models.ps1` 将模型准备到项目内的 `models/en-zh/`、`models/zh-en/`，供本地离线推理测试使用。

## 构建与打包

构建需要 Windows 11 x64、Rust 1.95 或更新版本的 MSVC 工具链、Visual Studio C++ 构建工具及 Windows SDK。运行默认构建需要兼容的 Windows App Runtime。

```powershell
git clone https://github.com/FluorescentLava/WinTranslator.git
cd WinTranslator
.\scripts\download-runtime.ps1
.\scripts\download-dictionary.ps1
.\scripts\build.ps1
```

构建脚本在项目根目录生成 `WinTranslater.exe`，运行时保留 `dictionary/` 和 `runtime/`；离线模型可在应用内下载或复用已有的 `models/`。`build.ps1 -SelfContained` 可附带 Windows App SDK 运行库，部署时需同时保留生成的运行库资源。原生 WinUI 3 封装位于 `vendor/windows-reactor`，依赖版本由 `Cargo.lock` 固定。

```powershell
cargo build --release --locked
cargo test --locked
# 使用项目内已下载的模型验证中英双向推理：
$env:LINGUA_MODELS_DIR = (Resolve-Path .\models).Path
cargo test --locked offline::tests::real_offline_translation_both_directions -- --ignored --exact
Remove-Item Env:LINGUA_MODELS_DIR
```

生成安装包需另行安装 [Inno Setup 6.5 或更新版本](https://jrsoftware.org/isdl.php)：

```powershell
.\scripts\package-release.ps1
# 指定 Inno Setup 编译器：
.\scripts\package-release.ps1 -IsccPath 'C:\Tools\Inno Setup 6\ISCC.exe'
```

打包脚本构建使用系统 Windows App Runtime 的版本，收集 ONNX Runtime、本地 ECDICT 词典和 Visual C++ DLL，在 `dist/` 生成安装程序。模型由应用内下载，打包无需准备模型文件。版本读取 `Cargo.toml` 并与 EXE 产品版本核对；安装配置在 `installer/WinTranslater.iss`，运行库来源在 `installer/runtime.json`。

可用 `-SkipBuild -TargetDirectory '<Cargo target 目录>'` 复用普通 Release 构建，自包含构建不用于此安装包。找不到 CRT 时，用 `-VcRuntimeDirectory '<x64\Microsoft.VC143.CRT 目录>'` 指定。用 `-KeepStage` 保留打包载荷后，可运行 `.\scripts\test-installer.ps1 -PayloadPath '<staging 目录>'` 检查运行库、词典、模型排除规则和文件完整性；安装器响应检查脚本为 `scripts/test-installer-responsiveness.ps1`。

源码构建也可通过脚本安装，或登记已有程序目录：

```powershell
.\scripts\install.ps1
.\scripts\register-app.ps1 -InstallDirectory "$env:USERPROFILE\Documents\WinTranslater"
```

`install.ps1` 默认使用与安装包相同的目录，可用 `-InstallDirectory` 指定用户目录内的独立 `WinTranslater` 文件夹。脚本安装可从系统设置或安装目录的 `scripts/uninstall.ps1` 卸载；只移除登记的程序文件与入口，保留用户数据及清单外文件。安装、登记和卸载脚本支持 `-WhatIf`。

## 本机数据与网络访问

设置、历史、免费用量估算、缓存、诊断记录及下载的离线模型保存在 `%LOCALAPPDATA%\TranslationTool`。API 凭据由 Windows DPAPI 为当前用户加密保存。读取或解密凭据失败时，程序保留原文件并阻止空配置覆盖；`WinTranslater.exe --check-configuration` 可生成包含路径、读取状态与凭据存在标记的本机报告，不输出密钥。

在线翻译会把文本发送给实际尝试的服务。中文词语的补充释义、未缓存的英语音标及短文本发音音频使用有道词典；离线模型下载访问 Hugging Face 上游仓库。本地词典、已下载的离线模型、已有缓存和本机语音在设备上执行。

仓库不包含个人凭据、设置、历史、编译产物、模型权重或运行库二进制文件；运行库与词典通过下载脚本准备，离线模型可在应用内或通过脚本下载。

## 许可证

应用源码采用 [MIT](LICENSE) 许可证。第三方依赖、词典、模型及服务标识遵循各自的许可与权利声明，详见 [第三方声明](THIRD_PARTY_NOTICES.md)、[应用图标来源](assets/icons/sources.md) 和 [服务图标来源](assets/providers/sources.md)。本项目与服务方没有隶属关系。
