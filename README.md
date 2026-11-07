# WinTranslator

用 Rust 编写的 Windows 翻译工具，界面使用原生 WinUI 3 和 Mica。

> 当前处于开发阶段。界面重设计、自动翻译、全局快捷键和后台启动正在整合与验收，仓库暂未发布可下载的正式版本。下列操作方式是本次版本的目标行为，最终以验收后的实现为准。

## 使用

- 按 `Ctrl+Tab` 唤出窗口，显示在当前屏幕工作区右下角。
- 顶部选择源语言和目标语言，下方输入单词或文本；停止输入后自动翻译，无需点击翻译按钮。
- 有结果时展开译文，服务图标表示实际提供该译文的服务；回退到本地时显示本地图标。
- 齿轮打开独立设置窗口，左侧切换设置分类。
- 开机自启时静默驻留后台，不弹出主窗口。可在设置中关闭自启。
- 关闭主窗口后保留后台运行，通过托盘菜单退出程序。

## 翻译服务

默认选择腾讯翻译。在线服务需要自行开通，并在设置中填写对应凭据。

| 服务 | 配置 |
| --- | --- |
| 腾讯云机器翻译 | SecretId、SecretKey、地域 |
| 百度翻译开放平台 | APP ID、密钥 |
| 有道智云 | 应用 ID、应用密钥 |
| DeepL | API Key，选择 Free 或 Pro |
| MyMemory | 无密钥试用，受服务方额度和使用限制约束 |
| 本地离线 | 下载运行库和中英双向模型 |

接口调用代码已实现；付费服务尚未使用有效账户凭据完成端到端验证。选择“腾讯”只代表首选服务，不代表请求已成功发送到腾讯。

启用自动离线后，缺少在线凭据或发生网络错误时会尝试本地翻译；鉴权、额度等服务错误会提示，不应被伪装为在线翻译成功。

## 离线翻译

使用 ONNX Runtime 和 OPUS-MT 量化模型，支持简体中文与英语互译。模型下载完成后，推理不需要联网，也不需要 Python。

首次准备，在项目根目录运行 PowerShell：

```powershell
.\scripts\download-runtime.ps1
.\scripts\download-models.ps1
```

下载脚本将运行库放入 `runtime/`，模型放入 `models/en-zh/` 和 `models/zh-en/`。模型下载会记录上游修订、文件大小和 SHA-256 校验信息。

本地模型适合日常短文本，单词、多义词和部分句子的译文仍可能不准确，不能视为词典释义。当前离线输入按行处理，每段最多 256 个输入 token；在线输入最多 5000 个字符。

## 构建

环境：

- Windows 11 x64。
- Rust 1.95 或更新版本，使用 MSVC 工具链。
- Visual Studio C++ 构建工具和 Windows SDK。
- Windows App Runtime 2.4 或更新的兼容 2.x 版本。

```powershell
git clone https://github.com/FluorescentLava/WinTranslator.git
cd WinTranslator
.\scripts\download-runtime.ps1
.\scripts\download-models.ps1
.\scripts\build.ps1
```

构建脚本在项目根目录生成 `翻译.exe`。运行时保留 `assets/`、`runtime/` 和 `models/` 等配套资源，不要只移动 EXE。

仅编译源码：

```powershell
cargo build --release --locked
```

测试：

```powershell
cargo test --locked
# 已下载模型和运行库后，包含真实离线推理测试：
cargo test --locked -- --include-ignored
```

## 本地数据

设置和历史记录保存在 `%LOCALAPPDATA%\TranslationTool`。API 凭据通过 Windows DPAPI 加密保存。在线翻译会把输入文本发送到所选服务；本地翻译在设备上执行。

仓库不包含个人密钥、设置、历史记录、编译产物、模型权重或运行库二进制文件。离线资源通过上述脚本下载。

## 代码

| 文件 | 内容 |
| --- | --- |
| `src/ui.rs` | 原生 WinUI 3 界面与交互 |
| `src/native.rs` | Windows 窗口及原生集成 |
| `src/api.rs` | 在线服务调用与回退 |
| `src/offline.rs`、`src/spm.rs` | ONNX 推理与分词 |
| `src/model.rs`、`src/storage.rs` | 数据结构与本地存储 |
| `assets/providers/` | 服务图标及来源记录 |

应用代码为 Rust，通过 `windows-reactor` 使用原生 WinUI 3 控件。

## 许可证

应用源码采用 [MIT](LICENSE) 许可证。

第三方依赖、模型和服务标识各自遵循原有许可及权利声明，详见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) 和 [图标来源](assets/providers/sources.md)。本项目与腾讯、百度、有道、DeepL 等服务方没有隶属关系。
