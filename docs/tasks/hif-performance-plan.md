# HIF 前台与后台加速计划

更新：2026-09-11。先保证缓存表示正确，再分别优化前台交互和后台持久化。

## 已接通的正确性基线

- [x] 锐化后的 display tiles 在完成通知后合成 full JPEG，并使用现有 `SharpeningState::Display` 保存；未锐化结果使用 `None`。
- [x] 开启锐化精确查询 Display，关闭锐化精确查询 None；命中后直接展示完整 artifact，不再次锐化。
- [x] Display 结果仅属于当前 HEIF full 显示请求，不进入按 `(path, level)` 索引的通用未锐化 preview projection。
- [x] 前台 FFmpeg 始终发布 JPEG tiles；后台仅持有 tile 的 Arc 引用，停留检测及独立许可通过后才进行整图合成。
- [x] session 完成后的缓存检查只查询，不再通过通用 full 队列触发第二次 HIF 解码。
- [x] 使用既有源版本、缓存 generation、临时文件和原子发布契约；补充布局校验、取消边界以及 256 MiB 组装缓冲预算。

兼容的 JPEG tiles 现已使用 libjpeg-turbo DCT 系数拼接，不做 IDCT、RGB/RGBA 重建或再次量化。首版接收 8-bit sequential YCbCr 4:4:4，包含 FFmpeg 的各分量 1×2 采样；带色度子采样的 JPEG 回退像素路径，避免新增边界插值差异。RGBA tiles，以及量化表、采样、iMCU 对齐或编码类型不兼容的 JPEG，也走像素组装回退；损坏、缺失或重叠的输入直接终止缓存写入。前台不等待后台步骤。

## P1：后台 full artifact 加速

- [x] 将 [DCT 系数拼接研究](../research/2026-09-09-hif-dct-stitch.md) 中的一次性拼接原型接入 oxy-media。已接入固定 Huffman 表模式；性能按下方生产适配验证记录解释。研究中的约 15% 体积差是固定表对优化表的比较。
- [x] 增加最小 libjpeg-turbo C adapter；错误与 longjmp 全部留在 C 边界，Rust 获取结构化错误。固定原生依赖版本、SIMD 构建和打包，并更新第三方说明。
- [x] 从 Arc JPEG memory source 读取，直接输出 ArtifactCache 临时文件；保留 sharpening、orientation、color，不增加“是否锐化”字段。
- [x] 在分配前检查画布、覆盖、重叠、实际采样因子、iMCU 对齐和各分量量化表。量化不兼容不得静默重新量化；允许受控回退当前基线。
- [x] 一个后台组装任务同时运行，按输出系数、最大单 tile 系数与 8 MiB 余量检查 256 MiB 预算。当前 33 MP 样本的系数预算约 219.6 MiB；不能把“没有 RGBA”当成低内存或零拷贝。
- [x] 在 tile、块行和编码进度边界处理取消；缓存清空、源修改和切图不得导致错误版本发布。
- [x] 记录生产路径的熵读取、系数重排、编码、cache commit、产物大小及原生应用进程峰值内存。
- [ ] 补充后台运行时切下一张的交替 A/B first/all tile p50/p95 矩阵，并把 fsync 从 commit 总时间中拆出。

验收：真实普通/锐化样本系数不变、方向覆盖正确；兼容样本逐像素一致。异常不产生可命中的半成品；后台耗时明显低于当前像素合成，且前台 p95 不回退。

## P2：前台 full 解码与交付加速

- [ ] 基于 [FFmpeg 进程复用研究](../research/2026-09-09-hif-ffmpeg-residency.md) 做独立常驻 libav worker 原型。先比较 CLI / 仅常驻 / 兼容上下文复用三组，不把 concat 吞吐当成单张收益。
- [ ] 采用逐请求输入，保持 JPEG tile 输出与已应用方向。首次按需启动或异步预热，禁止阻塞文件夹首屏。
- [x] 现有 CLI 接入逐 tile 编码完成即发布，并移除前台通用能力探测。相同本地 HIF 的三次 release/WebView2 冷测，首 tile 中位 1033→747 ms，全部 tile 1082→843 ms；首 tile 时实测 Canvas 已可见。见[分阶段数据](../research/hif-benchmark-data/streaming-jpeg-webview.json)。常驻 worker 的 20 次交替 A/B 验收仍待完成。
- [ ] 使用有界队列、generation、源版本和取消；当前选择优先，丢弃过期工作。worker 超时/崩溃可重建，CLI 保留回退。
- [ ] 前台 worker 与后台编码保持独立执行通道，不能让 full 缓存编码占据前台唯一队列。
- [ ] 保留 embedded preview 首绘；继续遵守 cold preview 800 ms、warm preview 150 ms 和目录首屏预算。

验收：至少 20 次交替 A/B，单张请求到 first/all tile 的中位改善至少 10% 或 50 ms，p95 不回退。必须在打包 Windows WebView 实测，区分本地/NAS、进程启动与文件缓存状态。

## P3：可选复用与完整矩阵

- [ ] 仅在 P1/P2 独立稳定后评估 native YUV 帧短暂复用是否仍有必要，避免同时保留完整像素与系数画布。
- [ ] 补足真实竖拍、不同网格、色度/位深、ICC、边界裁剪和损坏 HIF；受控旋转案例不能替代全部源元数据验证。
- [ ] 记录打开/关闭锐化、缓存恢复、快速切图、缓存清空和源修改的真实应用结果。

## 归档与当前验证

- [DCT 拼接报告](../research/2026-09-09-hif-dct-stitch.md)
- [FFmpeg 常驻与批处理报告](../research/2026-09-09-hif-ffmpeg-residency.md)
- [原始实验数据](../research/hif-benchmark-data/README.md)

正确性基线已通过真实 DSC01443.HIF 的 Display 持久化、None 隔离和缓存再读取不重复锐化测试；前端回归验证直接 img 显示与锐化切换重新查询。Rust fmt / workspace Clippy / workspace tests，以及前端 check / 159 tests / build 均通过。

2026-09-10 Release Windows WebView 实测（单次、本地样本、独立 data/cache）：冷启动全部 6 个 tiles 绘制成功，后端 decode+publish 665 ms；Display full 为 7008×4672、8,591,246 bytes。后台缓存约 45 秒后发布，包含等待与当前像素合成/重编码流程，尚未分阶段计时。重启应用后 full 请求到 cache hit 23.6 ms，到 full image load 120.6 ms，未创建 tile session。测试实例已关闭。这是正确性检查，不是 p50/p95 或性能预算认证。

原始记录：[cold WebView](../research/hif-benchmark-data/display-cache-cold-webview.json)、[warm WebView](../research/hif-benchmark-data/display-cache-warm-webview.json)。当时 warm HIF 性能场景改为要求完整 artifact 命中和加载，并为慢速像素基线保留 60 秒热身停留；下方记录 DCT 接入后的调整。

### DCT 接入后的验证（2026-09-10）

- libjpeg-turbo 3.1.3 静态 SIMD 构建；Windows Release 原生应用以三张 DSC01443/01444/01445.HIF 各两轮执行，共六次独立缓存生成和六次重启热命中。所有场景完成，无 DCT 回退，热命中均未创建 tile session。
- DCT 编码总时间中位 **459 ms**（444–483 ms）；其中熵读取约 236 ms、系数拷贝约 25 ms、熵编码约 131 ms，其余为头部检查、数组初始化及文件收尾。缓存提交中位 **27 ms**（25–93 ms），未单独拆 fsync。
- 瓦片绘制完成后约 **1.24–1.46 秒**可观察到磁盘 artifact，包含原有 1 秒 dwell 与 100 ms 轮询粒度。此前像素基线约 45 秒；该比较包含流程等待，不作为纯算法加速倍率。
- 热命中 full 请求到 cache hit 中位约 **22 ms**，到 full image load 中位约 **96 ms**。DSC01443 产物为 **6,147,592 bytes**，与先前研究的固定表产物逐字节一致。
- 原生应用进程峰值工作集约 262–264 MiB，DCT 结束后降至约 42–44 MiB；不包括独立 FFmpeg/WebView 子进程，不代表整个应用进程树峰值。系数预算为 230,240,256 bytes。
- 三张生产 artifact 均经独立工具逐块验证，每张 **98,224,128 个量化系数全部一致**；单测另对真实普通/锐化瓦片逐像素比较一致，并覆盖乱序、外边界裁剪、坏流、布局/量化不兼容、预算、I/O、多个阶段取消及临时文件清理。
- `cargo fmt --all --check`、workspace Clippy、workspace tests、Release 应用构建均通过；Windows 为本次实际运行平台，macOS/Linux 构建已接入 CI，尚未在本机执行。测试实例已关闭。

热身停留已由像素基线的 60 秒恢复为保守的 8 秒。后续仍需完善上方并发 p50/p95 及真实竖拍/其他相机矩阵。

记录：[六轮数据](../research/hif-benchmark-data/dct-production-results.json)、[生产系数验证](../research/hif-benchmark-data/dct-production-coefficients.json)、[cold WebView](../research/hif-benchmark-data/dct-production-cold-webview.json)、[warm WebView](../research/hif-benchmark-data/dct-production-warm-webview.json)。

补上 4:4:4 兼容限制后重新通过全仓库检查，并复验最终 Release：DCT 编码 429 ms、提交 33 ms、热缓存 full load 97 ms；产物 SHA-256 与上述已验证 DSC01443 输出相同。[最终复验](../research/hif-benchmark-data/dct-production-final-smoke.json)。
