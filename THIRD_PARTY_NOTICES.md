# Third-party notices

The application sources are distributed under the MIT license. Dependencies remain subject to their own licenses.

- **windows-reactor / windows-rs**, Microsoft: MIT OR Apache-2.0. Self-contained installers redistribute Microsoft Windows App SDK 2.4.0 under Microsoft's license; see `licenses/windows-app-sdk-LICENSE.txt` and `licenses/windows-app-sdk-NOTICE.txt`. Non-self-contained builds use the installed Windows App Runtime.
- **Microsoft.Web.WebView2.Core**, Microsoft: redistributed as part of the native WinUI runtime dependencies. See `licenses/webview2-LICENSE.txt` and `licenses/webview2-NOTICE.txt`. The application UI uses native WinUI controls.
- **Microsoft Visual C++ runtime**, Microsoft: app-local redistributable DLLs are copied from the installed Visual Studio Build Tools `VC/Redist/MSVC/x64/Microsoft.VC143.CRT` directory under the Visual Studio redistributable terms.
- **Inno Setup Simplified Chinese translation**, maintained by Zhenghan Yang: `installer/Languages/ChineseSimplified.isl` is from the official Inno Setup `is-6_7_3` source distribution, under the [Inno Setup license](https://github.com/jrsoftware/issrc/blob/is-6_7_3/license.txt). Inno Setup is used only to build the installer.
- **Language swap vector**: locally drawn parallel arrows with single-sided arrowheads, distributed under this repository's MIT license. The license for the previously used Microsoft Fluent UI System Icons vector is retained in `licenses/fluent-icons-MIT.txt`.
- **ONNX Runtime 1.24.4**, Microsoft: MIT. See `runtime/LICENSE.txt` and `licenses/onnxruntime-ThirdPartyNotices.txt`.
- **OPUS-MT Marian translation models**, Helsinki-NLP / OPUS, converted to ONNX by Xenova: Apache-2.0, as identified in the upstream model cards. See `licenses/Apache-2.0.txt`. The model manifest records the exact repository revision and file hashes.
- **Marian SentencePiece tokenizer construction**, adapted from Hugging Face Candle's `candle-examples/examples/marian-mt/spm.rs`: MIT. See `licenses/candle-MIT.txt`.
- **ort**: MIT OR Apache-2.0. **tokenizers**: Apache-2.0. **reqwest, serde, serde_json**: MIT OR Apache-2.0. **arboard**: MIT OR Apache-2.0. Other Rust transitive dependency versions are recorded in `Cargo.lock`.

Official translation services and their trademarks belong to the respective providers. This application is an independent client and is not affiliated with Baidu, Tencent, Youdao, DeepL, or Microsoft.


- **ECDICT**, skywind3000 and contributors: MIT. See dictionary/ECDICT-LICENSE.txt and dictionary/SOURCES.md. The downloaded CSV is excluded from source control.
- **Local windows-reactor adjustments**: vendor/windows-reactor preserves the upstream MIT and Apache-2.0 licenses. Changes expose per-TextBox font size, placeholder foreground, and resource overrides, plus an element-scoped caret color workaround for transparent Mica. The workaround identifies the native caret defensively and changes only its fill/composite mode; upstream licenses are preserved.
