# Third-party notices

The application sources are distributed under the MIT license. Dependencies remain subject to their own licenses.

- **windows-reactor / windows-rs**, Microsoft: MIT OR Apache-2.0. Native WinUI 3 is supplied by the installed Microsoft Windows App SDK runtime under Microsoft's license.
- **Fluent UI System Icons**, Microsoft: MIT. The 20px regular Arrow Swap vector is from `microsoft/fluentui-system-icons`, `assets/Arrow Swap/SVG/ic_fluent_arrow_swap_20_regular.svg`. See `licenses/fluent-icons-MIT.txt`.
- **ONNX Runtime 1.24.4**, Microsoft: MIT. See `runtime/LICENSE.txt` and `licenses/onnxruntime-ThirdPartyNotices.txt`.
- **OPUS-MT Marian translation models**, Helsinki-NLP / OPUS, converted to ONNX by Xenova: Apache-2.0, as identified in the upstream model cards. See `licenses/Apache-2.0.txt`. The model manifest records the exact repository revision and file hashes.
- **Marian SentencePiece tokenizer construction**, adapted from Hugging Face Candle's `candle-examples/examples/marian-mt/spm.rs`: MIT. See `licenses/candle-MIT.txt`.
- **ort**: MIT OR Apache-2.0. **tokenizers**: Apache-2.0. **reqwest, serde, serde_json**: MIT OR Apache-2.0. **arboard**: MIT OR Apache-2.0. Other Rust transitive dependency versions are recorded in `Cargo.lock`.

Official translation services and their trademarks belong to the respective providers. This application is an independent client and is not affiliated with Baidu, Tencent, Youdao, DeepL, or Microsoft.


- **ECDICT**, skywind3000 and contributors: MIT. See dictionary/ECDICT-LICENSE.txt and dictionary/SOURCES.md. The downloaded CSV is excluded from source control.
- **Local windows-reactor adjustments**: vendor/windows-reactor preserves the upstream MIT and Apache-2.0 licenses. Changes expose per-TextBox font size, placeholder foreground, and resource overrides, plus an element-scoped caret color workaround for transparent Mica. The workaround identifies the native caret defensively and changes only its fill/composite mode; upstream licenses are preserved.
