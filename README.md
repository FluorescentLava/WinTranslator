<p align="center">
  <img src="assets/icons/app.png" width="128" height="128" alt="WinTranslater 图标" />
</p>

# WinTranslater

用 Rust 编写的 Windows 翻译工具，使用原生 WinUI 3 控件与 Mica 背景。

## 下载

在 [GitHub Releases](https://github.com/FluorescentLava/WinTranslator/releases) 下载并运行 `WinTranslater-0.1.0-Beta-windows-x64-setup.exe`。需要 Windows 11 x64；安装包包含本地词典及中英双向离线模型，无需安装 Rust 或 Python。API 密钥需在设置中自行填写。

默认安装到 `%LOCALAPPDATA%\Programs\WinTranslater`，创建开始菜单入口和系统卸载项，无需管理员权限。安装包的 SHA-256 校验值见同一 Release 的 `SHA256SUMS.txt`。升级或卸载保留本机配置和 API 凭据。

程序使用系统已为当前用户注册的 Windows App Runtime 2.4 或更新的兼容 2.x x64 版本。安装程序先检查现有运行库；缺少兼容版本时才从微软官网下载官方安装程序，校验 SHA-256 和 Microsoft 数字签名，再安装并复查。已有兼容运行库时不下载、不重复安装，离线也可完成应用安装。运行库下载信息在 `installer/runtime.json` 中固定。

此前已安装附带 WinUI 运行库的安装包时，重新运行新版安装程序会按旧安装清单清理随包附带的 WinUI DLL 和各国语言目录；系统共享运行库由 Windows 维护，卸载应用时保留。

## 使用

- 按 `Ctrl+Tab` 唤出窗口，窗口移到当前虚拟桌面，并位于当前屏幕工作区右下角。在桌面 2 唤出时留在桌面 2。整块 Mica 面板与内容从任务栏下方滑入，失焦后沿原路滑下收起；使用原生 WinUI Composition 动画，唤出与隐藏时宽度保持固定。打开设置或服务选择浮层不会误隐藏。
- 在其他应用中划选文字后按 `Ctrl+Tab`，会先读取划选内容，再唤出窗口、填入输入栏并自动翻译。通过 Windows UI Automation 的文本选区接口读取，不触碰剪贴板；未选中文字或应用未提供该接口时照常唤出，保留原输入。
- 输入文本，停止输入 350ms 后自动翻译。中英语言对会跟随输入切换方向：输入中文时中译英，输入英文时英译中；选择其他语言后保留所选语言对。继续输入会取消旧任务，并保留上次结果等待更新。
- 单词和短句使用 23 DIP 字号；长段输入与译文使用 15 DIP 字号。输入区自动换行并随内容增高，达到高度上限后可在区域内滚动。
- 翻译成功后，结果卡片内才显示发音按钮。单词的词性位于上方，译文与英、美音标在下方同排显示；音标加载前后保持相同布局。只有喇叭按钮播放发音，卡片其余位置点击展开或收起释义。句子只显示一个喇叭。英式、美式音标独立读取词典的两种口音数据；相同音标照实显示，未查到的口音不借用另一种音标。音标异步查询并在本地缓存，不阻塞译文显示。
- 英语原文直接朗读原文，中译英时朗读英文结果；短词优先获取在线词典发音并在本地缓存。在线音频失败或文本超过 200 字符时，使用本机对应的 `en-GB`、`en-US` 语音完整朗读；未安装对应语音时明确提示，不用另一种口音替代。输入变化后隐藏旧结果的发音按钮，并停止旧文本的播放。
- 总词性位于译文上方，不再单独显示一份音标。点击卡片展开，再次点击收起。英文单词展示中文义项，包含主译文（例如 `n. 蛇`）；中文词语展开后列出更多可能的英文译词或短语及各自词性，不重复当前主译文。候选直接查询原中文词，先使用本地完整义项反查，再异步补充在线中英词典结果；补充结果缓存在本机，断网时可以复用。不会将“蛇皮”“蛇的”等词条扩成“蛇”的候选，带明确外语标记的词也不作为英文候选。词典未收录时不显示展开按钮，无法确认的词性以横线表示。
- 底部服务按钮打开独立的原生 WinUI Flyout，用于选择首选服务；展开列表不会改变主窗口大小，点击外部或按 Esc 收起。六项服务使用紧凑单列完整显示，不设滚动条，带官方服务图标及原生蓝色动画指示条。结果标识显示实际提供译文的服务；自动切换在线服务或转为离线时会相应更新。
- 点击右下角设置图标打开独立设置窗口，左侧切换分类；默认引擎按两行、每行三个排列，名称下显示配置状态，保留原生蓝色选择指示条。可填写 API 凭据、查看本程序估算的免费余量，并调整开机自启和自动离线。
- 历史页的搜索栏固定，记录列表独立滚动。
- 开机自启时静默驻留后台，首次唤出时才创建主窗口。关闭主窗口后继续驻留；通过托盘菜单退出。

可见面板宽 480 DIP，高度随内容变化；顶部语言框为 108 × 32 DIP，转换与设置按钮为 32 × 32 DIP。设置窗口为 760 × 540 DIP。面板高度变化带短暂过渡动画，底栏保持固定位置。

设置中的“用量明细”用原生进度条显示已用额度，同时显示 DeepL Developer 的累计用量和 DeepL API Free 旧版的本月用量；未配置的服务显示状态文字。在线路由的标题、服务顺序与切换说明在卡片下方分行排列。两种计划分别读取本机记录，查看明细不会修改所选计划或用量账本。

## 在线优先与免费额度

默认首选无需密钥的 MyMemory。在线路由先尝试当前所选且可用的免费服务，再依次尝试尚未尝试的百度、DeepL、腾讯、有道、MyMemory。有凭据的服务仅在已配置时加入路由；腾讯、有道还必须配置已核实且未到期的免费试用额度。

| 自动候选 | 配置 | 本程序采用的免费上限 |
| --- | --- | --- |
| 百度翻译标准版 | 自行申请并填写 APP ID、密钥 | 每月 50,000 字符 |
| DeepL Developer | API Key，选择 Developer | 总量 1,000,000 字符，不按月重置 |
| DeepL API Free 旧版 | 原有 API Key，选择旧版计划 | 每月 500,000 字符 |
| 腾讯翻译有效免费资源包 | SecretId、SecretKey，另行核实并配置免费资源包 | 仅配置的有限字符额度，到期即停，默认不启用 |
| 有道翻译有效免费试用 | 应用 ID、密钥，另行核实并配置免费试用额度 | 仅配置的有限字符额度，到期即停，默认不启用 |
| MyMemory | 无需密钥 | 匿名每天 5,000 字符 |

额度不足、限流、网络错误或鉴权失败时会继续尝试下一家在线服务。可用在线服务全部失败或在线等待预算耗尽后，启用“自动离线”才会调用本地模型；关闭该选项时显示错误。手动选择“本地离线”则直接在本地翻译。

腾讯已于 2026-10-01 停止新开通的免费资源，只有账户中仍有效的旧免费资源包可用；有道赠送额度仅用于试用。程序不会假定这两家每月赠送额度，也不会把现金余额视为免费字数。仅填写 API 凭据不会启用它们；没有核实有效免费字数和到期时间时会继续使用其他免费在线服务。DeepL 付费计划不加入路由。DeepL Developer 与旧版 Free 均使用 `api-free.deepl.com`；新 Developer 的总额度独立持久保存，用完后自动尝试其他免费在线服务。

腾讯、有道的免费试用设置分别是 `settings.json` 中的 `tencent_free_grant`、`youdao_free_grant`，默认均为 `null`。确认服务方账户显示的权益后，退出程序再配置对应对象；两家的字段结构相同，以下空值和 0 仍表示停用：

```json
{
  "tencent_free_grant": {
    "grant_id": "",
    "quota_characters": 0,
    "expires_at": ""
  },
  "youdao_free_grant": null
}
```

`grant_id` 是这份免费资源包或试用权益的稳定、唯一编号；`quota_characters` 是已核实可供本程序使用的免费字符总上限，必须大于 0；`expires_at` 是包含时区的绝对到期时间，采用 RFC 3339 格式（例如 `2026-12-08T23:59:59+08:00`，仅为格式示例）。仅看到有道体验金金额、尚不能确认可免费翻译的字数时，应保持停用。不要直接覆盖其他设置字段，也不要在此文件中填写 API 密钥。

同一试用编号的请求预留永久累计，不随重启、月份、修改上限或到期时间清零。首次使用后，该编号会绑定对应的 API 标识；修改密钥保留原用量，换成其他 API 标识会跳过这份额度。仅在服务方确实发放另一份独立免费权益后才填写新编号。每次预留以及每段请求前都检查有效期；收到服务方额度耗尽错误后，该试用编号不会自动恢复。设置页显示的试用剩余量包含失败请求的保守预留，属于本程序估算。

短文本的整轮在线等待预算为 6 秒，单次请求最多 3 秒；分段长文本的预算随段数增加，最高 30 秒。MyMemory 每段不超过 450 个 UTF-8 字节，百度每段不超过 1,000 字符并限制为每秒一次；请求间可取消。

免费字数在请求发出前持久预留，避免并发和重启重复使用本程序记录的额度。失败或取消的请求保守计入用量；额度、限流和临时错误分别冷却后重试。在线替代结果和离线回退结果不缓存，以便后续请求重新尝试恢复的首选服务。

这些余量和周期是本程序的本地保守估算，不代表服务账户的实时总额度，也不承诺精确计费。其他客户端的使用、服务方规则和实际结算周期可能影响可用额度；以服务方账户显示为准。百度超出免费额度可能收费，本程序按本地上限停止请求，仍需自行确认账户的计费设置。

## 本地词典与离线翻译

本地词典使用 ECDICT。离线翻译使用 ONNX Runtime 和 OPUS-MT 量化模型，目前支持简体中文与英语互译。资源下载后，词典查询和模型推理不需要联网或 Python。

在项目根目录通过 PowerShell 下载资源：

```powershell
.\scripts\download-runtime.ps1
.\scripts\download-models.ps1
.\scripts\download-dictionary.ps1
```

运行库位于 `runtime/`，模型位于 `models/en-zh/`、`models/zh-en/`，词典位于 `dictionary/`。模型下载记录上游修订、文件大小和 SHA-256 校验信息。

超过 5,000 字符时自动使用本地模型翻译全文。离线长文按句界、词界和模型 token 数顺序分段，保留段落；修改输入会取消尚未完成的任务。模型译文与词典释义是不同来源，多义词和部分句子的译文仍可能不准确。

长文自动转为本地后，仍可手动选择在线服务。此时只将前 5,000 个 Unicode 字符交给在线服务，并明确显示“在线仅翻译前5000字符，后续内容未翻译”。在线失败且启用自动离线时，本地会改译全文并更新提示。5,000 字符是本程序的在线处理上限，各服务的请求分段及免费额度仍分别适用。

## 构建与测试

构建环境：

- Windows 11 x64。
- Rust 1.95 或更新版本，使用 MSVC 工具链。
- Visual Studio C++ 构建工具和 Windows SDK。
- Windows App Runtime 2.4 或更新的兼容 2.x 版本。

```powershell
git clone https://github.com/FluorescentLava/WinTranslator.git
cd WinTranslator
.\scripts\download-runtime.ps1
.\scripts\download-models.ps1
.\scripts\download-dictionary.ps1
.\scripts\build.ps1
```

构建脚本在项目根目录生成 `WinTranslater.exe`。运行时保留 `dictionary/`、`runtime/` 和 `models/` 等配套资源。使用 `.\scripts\build.ps1 -SelfContained` 可同时放置 Windows App SDK 运行库及其资源，部署时需保留这些生成文件。

仅编译源码和执行测试：

```powershell
cargo build --release --locked
cargo test --locked
# 已下载模型和运行库后，包含真实离线推理测试：
cargo test --locked -- --include-ignored
```

界面使用本仓库 `vendor/windows-reactor` 中的原生 WinUI 3 封装；依赖版本由 `Cargo.lock` 固定。

## 生成 EXE 安装包

准备好上述资源和构建环境，再安装 [Inno Setup 6.5 或更新版本](https://jrsoftware.org/isdl.php)：

```powershell
.\scripts\package-release.ps1
# 自定义编译器位置：
.\scripts\package-release.ps1 -IsccPath 'C:\Tools\Inno Setup 6\ISCC.exe'
```

脚本自动构建使用系统 Windows App Runtime 的版本，校验模型文件的 SHA-256，收集应用需要的 Visual C++ DLL，在 `dist/` 生成安装程序及 `SHA256SUMS.txt`。版本读取 `Cargo.toml`，必须与 `app.rc` 的产品版本一致。Inno Setup 配置在 `installer/WinTranslater.iss`；运行库检查、下载和安装由安装程序处理。资源从构建输出和源码目录的明确清单收集，不读取用户配置目录，也不收集旧自包含构建残留的 WinUI 文件。

已有普通 Release 构建时，可传 `-SkipBuild -TargetDirectory '<Cargo target 目录>'`；脚本读取 EXE 内嵌 manifest，拒绝将自包含版本打进此安装包。Visual Studio 的 CRT 目录未自动找到时，可通过 `-VcRuntimeDirectory '<x64\Microsoft.VC143.CRT 目录>'` 指定。安装逻辑测试可运行 `.\scripts\test-installer.ps1`。

## 安装与卸载

Release 安装程序默认安装到 `%LOCALAPPDATA%\Programs\WinTranslater`，创建名称为 `WinTranslater` 的开始菜单入口，并登记 Windows“已安装的应用”中的卸载项。通过系统设置卸载，或运行安装目录内的 `unins000.exe`；卸载保留 `%LOCALAPPDATA%\TranslationTool` 中的配置、历史、凭据和用量记录。

从源码构建后，也可使用原有 PowerShell 脚本安装：

```powershell
.\scripts\install.ps1
```

默认安装目录为 `%LOCALAPPDATA%\Programs\WinTranslater`。也可使用 `-InstallDirectory` 指定用户目录中的独立 `WinTranslater` 文件夹。安装脚本创建名称为 `WinTranslater` 的开始菜单入口，并登记 Windows“已安装的应用”中的卸载项；无需管理员权限。

已有可运行的文件夹可原地登记，不必移动程序或重新下载模型：

```powershell
.\scripts\register-app.ps1 -InstallDirectory "$env:USERPROFILE\Documents\WinTranslater"
```

此方式安装的版本可从系统设置卸载，或运行安装目录内的 `scripts\uninstall.ps1`。脚本移除对应的开机自启、开始菜单入口及登记文件清单内的程序资源，保留本机配置和清单外的文件。安装、登记和卸载脚本均支持 `-WhatIf` 预览。

## 本地数据

设置、历史记录、免费用量估算和诊断记录保存在 `%LOCALAPPDATA%\TranslationTool`。API 凭据通过 Windows DPAPI 加密保存。在线路由会将文本发送给实际尝试的服务；本地释义查询和离线翻译在设备上执行。中文词语的补充释义及未缓存的英语单词音标会向有道词典查询；点击发音时，未缓存的短文本会向有道词典请求音频，缓存和本机语音可离线使用。

手动启动和后台唤出都在创建 WinUI 前加载用户配置，再交给主窗口；打开服务列表时会刷新凭据状态。读取、解密或解析凭据失败时保留原文件并阻止空设置覆盖。可使用 `WinTranslater.exe --check-configuration` 生成仅含路径、读取状态和凭据存在标记的本机检查报告，不会创建窗口或输出密钥。

仓库不包含个人密钥、设置、历史记录、编译产物、模型权重或运行库二进制文件。离线资源通过下载脚本准备。

## 许可证

应用源码采用 [MIT](LICENSE) 许可证。

第三方依赖、词典、模型和服务标识遵循各自的许可及权利声明，详见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) 和 [图标来源](assets/providers/sources.md)。本项目与服务方没有隶属关系。
