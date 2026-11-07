# 翻译服务图标来源

取得日期：2026-11-08。所有线上服务的图标均直接取自相应官方网站或该官网 HTML 引用的官方 CDN。PNG 建议以等比包含方式显示在 16–24 px 的图标槽中，避免拉伸与裁切。

| 文件 | 服务 | 官方页面 | 资源 URL | 尺寸与处理 |
| --- | --- | --- | --- | --- |
| `tencent.png` | 腾讯翻译君 | [腾讯翻译君](https://fanyi.qq.com/) | [官方资源](https://fanyi.qq.com/favicon.ico) | 32 × 32 PNG。官网 favicon URL 返回的内容实际为 PNG；按 PNG 原样保存。 |
| `baidu.png` | 百度翻译 | [百度翻译](https://fanyi.baidu.com/) | [官方资源](https://fanyi.baidu.com/favicon.ico) | 32 × 32 PNG。从官网 ICO 的唯一 32 × 32 帧无损解码为 RGBA PNG；原始文件保留为 baidu-source.ico。 |
| `youdao.png` | 有道翻译 | [有道翻译](https://fanyi.youdao.com/) | [官方资源](https://ydlunacommon-cdn.nosdn.127.net/31cf4b56e6c0b3af668aa079de1a898c.png) | 96 × 96 PNG。官网 HTML 的 rel="icon" 声明；官方网易 CDN，下载字节原样保存。 |
| `deepl.png` | DeepL | [DeepL](https://www.deepl.com/en) | [官方资源](https://www.deepl.com/favicon.ico) | 256 × 256 PNG。从官网 ICO 的 256 × 256 帧无损解码为 RGBA PNG；原始文件保留为 deepl-source.ico。另保留官方矢量 favicon deepl.svg。 |
| `mymemory.png` | MyMemory | [MyMemory](https://mymemory.translated.net/) | [官方资源](https://mymemory.translated.net/public/img/favicon-32x32.png) | 32 × 32 PNG。官网 HTML 的 rel="icon" 声明；下载字节原样保存。 |
| `local.png` | 本地翻译 | 本项目 | 本地原创 | 96 × 96 PNG / 24 × 24 SVG。本项目绘制的显示器与处理器图形，用于表示在本机执行的翻译；它不是任何外部服务的商标。 |

`deepl.svg` 的官方来源为 <https://www.deepl.com/icon>，由 DeepL 官网声明为 SVG favicon，原始 `viewBox="0 0 144 144"` 保留；适用于方形图标槽。`local.svg` 为本项目自绘，`viewBox="0 0 24 24"`。PNG 版本可供原生 ImageIcon 直接加载。

服务图形的颜色、比例与轮廓均来自官方资源；没有用自绘字母或通用图形替代服务标志。腾讯官方 favicon 的译字、百度官方译字、有道官方 yd 以及 MyMemory 的大括号都是原始图标的一部分。

商标及原始图像权利属于各服务所有者。此来源记录不将服务商标重新授权为项目 MIT 许可证。

## 验证

已逐个解码所有 PNG，并检查 16 px、24 px、64 px 的预览。SVG 已通过 XML 解析并检查 viewBox，保留图形边界。DeepL PNG 使用独立图形标志以保证小尺寸可辨识。

## SHA-256

| 文件 | SHA-256 |
| --- | --- |
| `tencent.png` | `bd83bdb4943f21452edf6f0e3006f526690f9a6fc21405c56ba00252265e318b` |
| `baidu.png` | `ac2136eddcc6f75cf970e987362933d8f55a68e3f8e20167549b4e77d78ad82d` |
| `baidu-source.ico` | `53b41a891c20e43c2dcc566d179105310b763d5b873249c1b0b151aa6f082972` |
| `youdao.png` | `16166653573a5f02885a7a810ecbf535e592325947324a548e92ea93ebb96067` |
| `deepl.png` | `d0f302bf23bbacf3cc4f7e105fb342d554d8dd93c171d6328ec361ef02cd2a41` |
| `deepl.svg` | `e92ac3065c3121995fedd9b78609e915512af1da0fdd7865abf9f03bc56e1d41` |
| `deepl-source.ico` | `3ad4596ec2678a3eec891689ef105647e8366a78025ec1c224beea19c678bd54` |
| `mymemory.png` | `9f30401d9f9f2f43779a579eaf525192d46284c11ec430f42687cf69c8f5f778` |
| `local.png` | `2ec4b0bca642ab2ce27dd5f5a31419757f2901ee56bf6ec8ac80c9038716f8da` |
| `local.svg` | `46b515f1b8b0256796d31283104e824a83ac818f4ee70025421948bc5fcef23c` |
