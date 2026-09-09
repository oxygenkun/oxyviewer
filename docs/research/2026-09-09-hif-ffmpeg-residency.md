# HIF full 常驻解码验证与实现方案

日期：2026-09-09。归档于 2026-09-10；本文保留实验当时的结论，当前实施状态见 [HIF 加速计划](../tasks/hif-performance-plan.md)。结论：有优化空间，但普通 ffmpeg.exe 常驻/concat 批处理不适合直接替代交互式 full tile 请求。推荐下一阶段验证基于 libav 的请求式常驻 worker；本轮未修改生产代码，未实现该 worker。

## 实测条件

- Windows，本机实际解析到的 `<FFMPEG_BIN>/ffmpeg.exe`，9.0.1-full_build-www.gyan.dev。直接启动真实 exe，没有 Scoop shim 或 shell 开销。
- 与生产保持一致的 CREATE_NO_WINDOW、HIGH_PRIORITY_CLASS、输入 `-threads 2`，JPEG yuvj444p / mjpeg / qscale 2；锐化使用相同 unsharp 参数。
- 用户 NAS 目录 `<NAS_FIXTURE_DIR>` 中 DSC01443.HIF、DSC01444.HIF、DSC01445.HIF，只读复制到实验目录。
- 7008×4672，六个 HEVC 网格分量；本次仅验证这三张同构横向样本，没有验证不同网格、竖向和其他相机文件。
- 主对比使用本地副本，缓存可热，没有清空操作系统缓存。进程每次新建不等于物理存储冷读。
- 每种输出做四轮交替次序：单独进程每轮三张，concat 每轮一个进程三张。生产只处理一帧；concat 使用同样参数、passthrough 时间戳，允许连续帧。两者调度并不完全相同。
- 六 tiles 与整图 JPEG 两条路线分开测，未同时执行。整图路线使用生产相同 xstack/crop/yuvj444p，不经过 Rust RGBA。

## 数据

| 测量 | 每张新进程 | 三张同进程连续处理（平均每张） | 吞吐变化 |
|---|---:|---:|---:|
| JPEG tiles | 598.6 ms | 315.4 ms | 每张耗时减少 47.3% |
| 锐化 JPEG tiles | 617.9 ms | 334.9 ms | 每张耗时减少 45.8% |
| canonical 整图 JPEG | 2396.1 ms | 1401.1 ms | 每张耗时减少 41.5% |

单进程列是 12 个独立进程耗时的中位数；批处理列是四次整批耗时除以三后的中位数。包含进程启动、解码、过滤、编码、本地文件写入和退出，不包含单独的 ffprobe、Rust IPC 或 WebView 绘制。所有三张、所有 tiles、锐化和整图输出均完成逐字节一致性校验。

启动基线：`ffmpeg -version` 首次 101.4 ms；随后 30 次中位数 90.8 ms、p95 103.2 ms。16×16 lavfi 单帧空输出中位数 99.7 ms。此数据是启动/初始化/打印/退出的合计，不能当作纯 CreateProcess 时间，也不能保证 worker 恰好省下 91 ms。它约为单张 tiles 路径的 15%，整图路径的 4%。批处理的大幅收益不能全部归因于启动省时，还包含初始化摊销与流水线调度差异。

### 首张延迟反证

另做三轮观察 JPEG 文件结束标记（2 ms 轮询，非 WebView 时间）：

| 首张输出 | 单独进程 | 三张同进程批处理 |
|---|---:|---:|
| 第一个完整 tile JPEG | 459.3 ms | 689.3 ms |
| 第一张的六个完整 tile JPEG | 541.0 ms | 797.0 ms |
| 第一张完整 canonical JPEG | 2376.8 ms | 3805.2 ms |

批处理吞吐更高，首张交互延迟更差。生产当前还会等 FFmpeg 退出后才读取 tiles，不能把文件出现时间当成生产首 tile 发布。

stdin 验证：把一张 HIF 的合法 concat 列表写入并 flush，保持输入打开两秒，进程仍活着但零 JPEG；关闭 stdin 后约 535.9 ms 输出六个 JPEG并正常退出。因此这种方式不是可逐请求服务的常驻接口。

直接 NAS 单张 tiles：第一轮三张 713/686/727 ms，第二轮 588/572/583 ms。没有清空客户端或 NAS 缓存，不能称为严格冷/热盘实验；只说明文件读取状态会影响端到端时间。

## 方案选择

1. 不采用“保持普通 ffmpeg.exe 不退出，再不断输入文件名”。CLI 的输入、输出和过滤图来自启动参数；交互控制不是新解码任务协议。[FFmpeg CLI 文档](https://ffmpeg.org/ffmpeg.html)
2. 不把 concat 作为前台 full 默认路线。它需要同构 streams，初始化解析输入列表，且本次首张延迟回退。[concat 格式文档](https://ffmpeg.org/ffmpeg-formats.html#concat-1)
3. 候选是独立常驻 `oxy-heif-worker`，内部直接使用 libavformat/libavcodec/libavutil/libswscale（锐化需要 libavfilter 或经验证的等价实现）。当前机器安装只有 CLI exe，没有可直接链接的开发库；需要固定版本的库、headers 和可复现打包。仅用常驻父进程包装每次新建 ffmpeg.exe 不会消除现有启动成本。

## 分阶段实现设计

### 第一阶段：最小常驻 worker 性能原型

先在实验目录构建，不接主应用。一次只处理一张，禁止预读下一请求，测请求到达至首 tile/全部 tiles，避免用批处理吞吐代替交互收益。

对照三组：A 现有 CLI；B 常驻 worker，但每张新建 demux/codec/filter/encoder context；C 在 B 上仅复用兼容的 codec、转换器和缓冲池。B 隔离进程和动态库装载收益，C 测上下文复用的增量。固定线程上限、同样 JPEG 参数，覆盖本地和 NAS。

每个 HIF 仍有独立容器与源版本，必须重开/检查文件；常驻不意味着不用读取和解析。解码器复用必须检查 codec/profile、bit depth、chroma、尺寸和 extradata；不兼容就重建，兼容切换也要正确 flush。[avcodec_flush_buffers 文档](https://ffmpeg.org/doxygen/trunk/group__lavc__misc.html)

先至少 20 次单张到达实验，随机 A/B 顺序并保留原始记录；明确测 worker 首次启动和预热后的请求。选择值得集成的门槛：预热后 first/all tile 中位数改善至少 10% 或 50 ms，p95 不回退。门槛是建议验收目标，不是现有实测收益承诺。

### 第二阶段：接入现有会话契约

- 进程按首次 HIF 需求懒启动，可在 HIF 选择后异步预热；不能阻塞文件夹首屏。进程管理与解码在 oxy-media，Tauri 只接命令和事件。
- 本地匿名管道/命名管道使用长度前缀二进制协议，元数据包含 protocolVersion、requestId、sessionId、generation、sourceRevision、sharpening；结果为 Tile、Complete、Cancelled、Error、Ready。
- JPEG bytes 使用二进制传输，不进入 JSON IPC。复用现有 HeifTileData::Jpeg 与 WebView tile 协议。每个 tile 完成编码就可发布，不等待全部输出和进程退出。
- 仍逐 tile 处理，保持方向、裁剪、坐标、预期 tile 数和色彩事实；锐化只作用于显示副本。禁止为了缓存先生成整图 RGBA。
- 前台队列只保留当前 generation；过期请求不发布。控制通道必须能在执行解码期间接收取消，tile 边界检查取消，文件 I/O 使用中断回调。卡死超时终止并重建 worker；恢复当前 CLI 后端计划，不能无限重试损坏文件。
- 初版保持“前台常驻 worker + 后台独立 artifact 路线”。后台转码不能占据前台 worker 的唯一工作队列；这样先验证前台收益，避免同时改变缓存和内存生命周期。
- 记录 worker 启动、输入打开、probe、排队、解码、编码、首 tile/全部 tile、传输、WebView 首绘/全绘和内存峰值。

### 第三阶段：可选消除 canonical 二次解码

仅在第二阶段稳定后评估。短暂保留当前源版本的未锐化 native YUV AVFrame 引用，前台 JPEG tiles 发布后，经停留检测和后台许可再合成/编码 canonical；不要从锐化 JPEG 重建 canonical。

必须设置字节预算和引用生命周期，切图、取消、源修改或预算压力时释放。10-bit 4:2:2 的六张 3520×1600 原生 tile 数据就约 129 MiB，另有解码器/转换器开销；不能无界保留或默认宣称低内存。后台整图编码需独立执行通道；如果无法满足前台优先/内存预算，就放弃帧复用，继续既有独立源转码。

## 验收与当前边界

- 回归：横/竖向、不同网格和色度、锐化开/关、缓存命中、坏文件、源替换、快速切图、worker 崩溃、取消与关闭。
- 图像：三个现有样本要求输出尺寸/覆盖/方向一致；同一 FFmpeg 参数下尽量逐字节一致，若库 API 路径无法保证字节一致，需说明并做像素差异和色彩验证。
- 性能：打包 Windows WebView 的 first/all tile 绘制才是最终指标，同时不回退文件夹首屏、缩略图和内存预算。
- 本轮证明了启动开销和同进程批处理吞吐收益，也证明了 concat 首张回退与 stdin 限制；尚未测得请求式 libav worker 的收益，不承诺 42%–47% 的单张加速。
- 本轮没有修改生产代码，没有启动桌面 debug 实例；所有实验 FFmpeg 子进程已退出。

## 可复现材料

- `bench.py`、`results.json`：四轮本地性能与逐字节一致性检查。
- `latency.py`、`latency-results.json`、`stdin-results.json`：首张文件完成时间与开放 stdin 实验。
- `nas.py`、`nas-results.json`：NAS 读取对比。
- 上级目录 `ffmpeg-startup-benchmark.json`：30 次启动基线。


归档数据见 [实验数据索引](hif-benchmark-data/README.md)。原型、图片和编译工具仍保留在本次本地实验目录，不纳入仓库。
