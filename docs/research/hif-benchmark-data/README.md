# HIF 实验数据（2026-09-09）

- [DCT 拼接报告](../2026-09-09-hif-dct-stitch.md)：[性能样本](dct-results.json)、[系数和像素验证](dct-verification.json)、[旋转及负例](dct-extra-results.json)。
- [FFmpeg 常驻研究](../2026-09-09-hif-ffmpeg-residency.md)：[批处理样本](ffmpeg-results.json)、[首张延迟](ffmpeg-latency-results.json)、[stdin 实验](ffmpeg-stdin-results.json)、[NAS 对照](ffmpeg-nas-results.json)、[启动基线](ffmpeg-startup-results.json)。

记录保留实验命令、参数与测量值；私人路径已替换为 `<BENCH_ROOT>`（实验目录）、`<FFMPEG_BIN>`（工具目录）、`<NAS_FIXTURE_DIR>`（NAS 样本目录）等占位符，复现时需替换为自己的目录。数字不代表当前生产版本或 WebView 性能。样本为 DSC01443/DSC01444/DSC01445.HIF；图片及下载的第三方编译材料不归档到 git。

2026-09-10 Display 缓存正确性基线的真实 Release WebView 记录：[首次 tiles](display-cache-cold-webview.json)、[重启后 full artifact](display-cache-warm-webview.json)。它们与上述 DCT/FFmpeg 独立原型实验分开解释，详见 [当前验证](../../tasks/hif-performance-plan.md#归档与当前验证)。

DCT 接入后的生产路径：[六轮耗时/内存/缓存数据](dct-production-results.json)、[三张产物系数验证](dct-production-coefficients.json)、代表性 [cold WebView](dct-production-cold-webview.json) / [warm WebView](dct-production-warm-webview.json)。系数验证文件里的耗时和峰值属于独立验证工具；应用性能以六轮数据为准。
