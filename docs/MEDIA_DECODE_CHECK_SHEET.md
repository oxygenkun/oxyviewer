# 媒体格式解码检查表

本表按“格式 × 清晰等级”列出当前实现的解码/显示方式。单元格内自上而下表示回退顺序；上一项不可用或解码失败时才尝试下一项。

清晰等级：

- `thumbnail`：列表、网格和胶片栏使用的最快表示。
- `preview`：进入放大镜后的适窗表示。
- `full`：用于像素检查的最佳可用表示。

> `WebView 原文件`表示不经过 OxyViewer 的生成式解码流水线。缓存命中会直接返回已有产物，不列入解码回退顺序。

| 格式与清晰等级 | Windows | macOS | Linux |
| --- | --- | --- | --- |
| **JPEG/JPG** | WebView 原文件 | WebView 原文件 | WebView 原文件 |
| **PNG** | WebView 原文件 | WebView 原文件 | WebView 原文件 |
| **WebP** | WebView 原文件 | WebView 原文件 | WebView 原文件 |
| **RAW**（ARW/CR2/CR3/NEF/DNG/RAF/RW2/ORF）· `thumbnail` | LibRaw 内嵌 JPEG / bitmap（目标 512）<br>↓<br>LibRaw half-size development | LibRaw 内嵌 JPEG / bitmap（目标 512）<br>↓<br>Apple ImageIO JPEG（512）<br>↓<br>Apple Core Image RAW JPEG（512）<br>↓<br>LibRaw half-size development | LibRaw 内嵌 JPEG / bitmap（目标 512）<br>↓<br>LibRaw half-size development |
| **RAW**（ARW/CR2/CR3/NEF/DNG/RAF/RW2/ORF）· `preview` | LibRaw 内嵌 JPEG / bitmap（目标 4096）<br>↓<br>LibRaw half-size development | LibRaw 内嵌 JPEG / bitmap（目标 4096）<br>↓<br>Apple Core Image RAW JPEG（4096）<br>↓<br>Apple ImageIO JPEG（4096）<br>↓<br>LibRaw half-size development | LibRaw 内嵌 JPEG / bitmap（目标 4096）<br>↓<br>LibRaw half-size development |
| **RAW**（ARW/CR2/CR3/NEF/DNG/RAF/RW2/ORF）· `full` | LibRaw 近全尺寸内嵌 JPEG（覆盖源显示尺寸至少 90%）<br>↓<br>合格的 Windows WIC RAW full JPEG<br>↓<br>LibRaw full development | LibRaw 近全尺寸内嵌 JPEG（覆盖源显示尺寸至少 90%）<br>↓<br>Apple Core Image RAW full JPEG<br>↓<br>Apple ImageIO full JPEG<br>↓<br>LibRaw full development | LibRaw 近全尺寸内嵌 JPEG（覆盖源显示尺寸至少 90%）<br>↓<br>LibRaw full development |
| **HEIF/HEIC/HIF** · `thumbnail` | 已验证的 Sony HIF 内嵌 JPEG（160×120）<br>↓<br>FFmpeg auxiliary preview（512）<br>↓<br>libheif scaled preview | 已验证的 Sony HIF 内嵌 JPEG（160×120）<br>↓<br>Apple ImageIO（512）<br>↓<br>libheif scaled preview | 已验证的 Sony HIF 内嵌 JPEG（160×120）<br>↓<br>FFmpeg auxiliary preview（512）<br>↓<br>libheif scaled preview |
| **HEIF/HEIC/HIF** · `preview` | 已验证的 Sony HIF 内嵌 JPEG（160×120）<br>↓<br>FFmpeg auxiliary preview（4096）<br>↓<br>libheif scaled preview | 已验证的 Sony HIF 内嵌 JPEG（160×120）<br>↓<br>Apple ImageIO（4096）<br>↓<br>libheif scaled preview | 已验证的 Sony HIF 内嵌 JPEG（160×120）<br>↓<br>FFmpeg auxiliary preview（4096）<br>↓<br>libheif scaled preview |
| **HEIF/HEIC/HIF** · `full` | **FFmpeg 可用：**<br>FFmpeg JPEG tile-grid<br>↓<br>FFmpeg RGBA fallback<br>↓<br>libheif full decode<br><br>**FFmpeg 不可用：**<br>Windows WIC（仅开启硬件加速且该文件受支持）<br>↓<br>libheif full decode | Apple ImageIO full JPEG<br>↓<br>FFmpeg full JPEG<br>↓<br>Apple ImageIO 8192 preview<br>↓<br>libheif 8192 preview | FFmpeg full tile session<br>↓<br>libheif full decode |
| **TIFF** · `thumbnail` | 不支持：Windows native preview 尚未实现 | Apple ImageIO JPEG 512 | 不支持：Linux native preview 尚未实现 |
| **TIFF** · `preview` | 不支持：Windows native preview 尚未实现 | Apple ImageIO JPEG 512 | 不支持：Linux native preview 尚未实现 |
| **TIFF** · `full` | 不支持：Windows native preview 尚未实现 | Apple ImageIO JPEG 4096（当前最佳可用表示） | 不支持：Linux native preview 尚未实现 |

## FFmpeg 缺失时的检查重点

- JPEG、PNG、WebP、RAW 和 macOS TIFF 路径不依赖 FFmpeg。
- HEIF/HIF 的 `thumbnail` 与 `preview` 会跳过 FFmpeg并回退到 `libheif`；已验证的 Sony 160×120 内嵌 JPEG不受影响。
- Windows HEIF `full` 在开启硬件加速时可先尝试 WIC，否则回退到 `libheif`。
- Linux HEIF `full` 直接回退到 `libheif`。
- macOS HEIF `full` 首选 ImageIO；只有 ImageIO 失败时才涉及 FFmpeg及后续回退。
- FFmpeg 后端同时要求兼容的 `ffmpeg`、`ffprobe`、HEVC decoder，以及 `ffprobe -show_stream_groups` 支持。

## 发布检查

- [ ] Windows 安装包在没有系统 FFmpeg 的干净环境中完成全部格式与等级检查。
- [ ] macOS 安装包在没有 Homebrew FFmpeg 的干净环境中完成全部格式与等级检查。
- [ ] Linux 安装包在没有系统 FFmpeg 的干净环境中完成全部格式与等级检查。
- [ ] 按 [FFmpeg 打包说明](FFMPEG_PACKAGING.md) 构建，确认安装包内的 `oxy-ffmpeg` 与 `oxy-ffprobe` 成对存在，清空 PATH 后校验通过，并包含对应源码和许可证。`OXY_FFMPEG_DIR` 仅作显式诊断覆盖。
- [ ] 使用至少一个普通 HEIF、Sony HIF 和每个目标 RAW 厂商 fixture 验证实际回退顺序。
- [ ] 对最终失败记录完整 backend attempt diagnostics，而不是只记录最后一个错误。
