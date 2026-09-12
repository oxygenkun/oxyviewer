# JPEG thumbnail 与滚动验收（Windows，2026-09-12）

对应[实施计划](../tasks/jpg-quality-and-scroll-performance.md)与[表示规划设计](../architecture/08-representation-planning.md)。

## 实现范围

- JPEG thumbnail 从经关联、几何和色彩检查的 MPF 预览生成；不合格时回退主图。读取有界，JPEG full 继续使用完整主图。
- 新的 JPEG/RAW thumbnail 与 PNG/WebP 小图交付最长边不超过 512、编码不超过 2 MiB。JPEG 以质量 90 编码；PNG/WebP 保留 alpha。
- JPEG 使用既有静态 libjpeg-turbo 的 IDCT 缩放。新规范化路径共享两任务、估算 256 MiB 转换许可，按输入及后端缓冲计费；这不是整个应用或既有 RAW/HEIF full 解码器的内存上限。
- 浏览器直接保留小 Blob 和已解码 Image，删除大图 Canvas 转码。原生媒体协议在后台读取；完整 DOM 图片异步解码完成后才切换显示。
- Filmstrip 从 Loupe 主图状态拆出，稳定 item 与可见集合、按帧合并滚轮位移。元数据保留未变资产引用并按资产通知；标签读取批处理并移至后台数据库线程。
- 沿用源版本、缓存 generation、投影策略、lease、取消和整目录缩略图保留；旧超限 thumbnail 投影不能直接恢复。

## 方法与边界

使用真实 Release/WebView2，两个独立 NAS 目录：Sony 241 JPG + 241 ARW，Lumix 69 JPG + 69 RW2。
各做三轮 grid/filmstrip × 冷缓存/复用缓存，共 24 次应用启动，每次增加全目录预热后的反向热滚动。
冷缓存使用独立 data/cache/WebView profile；复用缓存仍是新进程和新 data，复用已生成 artifact 及 WebView profile。
没有清空 OS/SMB cache，因此不称为严格冷盘。输入为 CDP wheel（`isTrusted=true`），不是硬件滚轮测量。

通用探针：`node scripts/perf-scroll.mjs PORT OUTPUT --steps=400 [--filmstrip]`；
随后 `--reverse --await-retained=COUNT [--filmstrip]`。应用使用 `OXY_PERF_SCENARIO` 的真实目录，
grid 等待 `harness:first-page-painted`，filmstrip 选择 JPEG 并等待 `image:loaded@full`。
探针在首个 contentful paint 后开始滚动，同时保留完整原始帧序列、启动帧与长任务。
24 次样本不启用 CPU profiler；定位 trace 单列。截图、原始 JSON、trace、外部照片均在 ignored 报告目录，不入库。

帧间隔来自 rAF，不能等同于每帧已被显示器呈现。可见图片 ready 和截图在每段结束时核对，
不把这项证据扩大为滚动期间每帧都没有占位图。输入延迟按页面 wheel 事件到 scroll 位移计算，
边界无位移输入不计；不跨宿主和浏览器时钟相减。

## 结果

原始构建使用相同 CDP 探针、Sony 目录、400 次 filmstrip wheel 复测：滚动阶段有 10 个
长任务，最大 476 ms，最大帧间隔 481.3 ms，17 次 Canvas 转换。新路径的 24 次应用启动、
48 段采样均没有滚动阶段长任务或 Canvas 转换。较低 P95 不代表没有长帧，下面保留最大值。

| 场景 | 冷缓存最大帧间隔 | 复用磁盘缓存最大帧间隔 | 全目录热滚动最大帧间隔 |
| --- | ---: | ---: | ---: |
| Sony grid | 18.8 ms | 87.5 ms | 12.6 ms |
| Sony filmstrip | 125.0 ms | 112.4 ms | 18.7 ms |
| Lumix grid | 18.7 ms | 50.0 ms | 6.7 ms |
| Lumix filmstrip | 81.3 ms | 74.9 ms | 6.5 ms |

各组 P95 为 6.3–6.4 ms，但首次完整图加载期间仍有上述长帧；**不能宣称所有卡顿均已消失**。

每段结束时可见缩略图全部 ready，交付与保留的 Blob 图片最长边均不超过 512。
全目录保留后再执行反向热滚动：新增 native thumbnail 请求、Image decode 和 Canvas 转换全部为 0。
Sony 保留 482 张，RGBA 估算约 322.0 MiB；Lumix 保留 138 张，约 91.9 MiB。
这些是按像素计算的保留量，不等于浏览器真实工作集。初轮实测各进程峰值分别为：

| 样本 | Native | WebView 主进程 | Renderer | GPU 进程 |
| --- | ---: | ---: | ---: | ---: |
| Sony | 199.5 MiB | 224.4 MiB | 451.4 MiB | 273.4 MiB |
| Lumix | 157.0 MiB | 194.2 MiB | 335.9 MiB | 223.7 MiB |

这里取 OS `PeakWorkingSet64`；不同进程的峰值不一定发生在同一时刻，不能相加称为同时峰值。
所有热滚动读 lease 为 0；grid 没有 UI lease，filmstrip 保留当时仍有显示消费者的描述符。
后台转换许可不会驱逐已保留的整目录小图。

后续验收还修复了 full 描述符在 thumbnail 显示前被提前释放的问题，以及 RAW Interim
被误报为完整图的问题。Grid 测量使用 frontend `index-D1VQrMKc.js`；仅影响 Loupe 的这两项
修复后，使用最终 `index-DoWveEI4.js` 对全部 12 个 filmstrip 冷／热场景重新跑了三轮。
原始测量没有覆盖或删除。最终 Release SHA-256：
`53ea6fca9445562b207df433b637fb81c899f943ce22ec53ad5f2a9d7644d527`。

### 真实照片读取与几何

| 输入 | 主图编码尺寸 | MPF 输入 | 实际读取量／次数 | 小图交付 |
| --- | --- | --- | --- | --- |
| Sony JPEG | 7008×4672 | 1616×1080，137,496 B | 465,176 B／6 次 | 342×512，32,274 B |
| Lumix JPEG | 6000×4000 | 1620×1080，829,984 B | 1,092,128 B／5 次 | 341×512，48,767 B |

读取计数位于 `BufReader` 下方，包含必要头部、候选前缀和 read-ahead；是应用读取字节数，
不是物理 SMB 网络流量。两个外部 fixture 测试确实执行，均断言读取少于原文件的四分之一。
原图按 EXIF 方向缩到同尺寸后，与新小图逐通道平均绝对 RGB 差值分别为
Sony `(1.43, 0.83, 1.60)`、Lumix `(1.97, 1.47, 2.21)`，单位为 8-bit 通道值。
这项小图对照只验证当前样本的方向、内容与色彩近似，不是 RAW 100% 画质认证。

真实 WebView 中完整 JPEG 分别显示 4672×7008、4000×6000，方向和小图内容一致。
Sony RAW 从 342×512 Interim 升到 4672×7008，完整资源的 1,632,957 字节与 LibRaw 选出的最大
内嵌 JPEG 完全一致。Lumix RW2 的内嵌 JPEG 不足以覆盖源尺寸，按既有策略开发 RAW；
最终实际上屏 4008×6008。它不适用“有完整尺寸内嵌 JPEG”的 Sony 专用 fixture 断言。

### 本地 SSD 三轮门槛

下表使用最终构建、新进程和隔离状态。暖缓存复用 artifact 与 WebView profile；没有清 OS cache。
默认 runner 以三次中位数检查预算，同时保留 p95（在三次样本中即最大值）。

| 场景 | 首预览中位数／最大值 | 门槛 |
| --- | ---: | --- |
| Sony JPEG cold | 284.0／296.7 ms | 中位数通过 800 ms |
| Sony JPEG warm | 85.7／275.1 ms | 中位数通过 150 ms，单次尾部超标 |
| Lumix JPEG cold | 240.4／260.5 ms | 中位数通过 800 ms |
| Lumix JPEG warm | 94.0／109.4 ms | 通过 150 ms |
| HIF cold | 99.8／100.1 ms | 通过 800 ms |
| HIF warm artifact | 98.5／127.1 ms | 通过 150 ms |

完整图出现时间另外记录：Sony JPEG cold 中位数 615.3 ms、warm 604.0 ms；Lumix cold
455.1 ms、warm 442.2 ms。RW2 单次首预览 122.3 ms，完整开发显示 107.5 s；这是未改写的 RAW
开发后端成本。`Interim` 小图现在只报告预览状态，不能提前标为“完整解析完成”或 full 上屏。
随后复用同一磁盘缓存、使用新进程和新 WebView profile，实机确认 RW2 直接恢复
4008×6008 完整图，方向和画面正常；检查时 native 累计 CPU 约 0.48 s，没有再次执行完整开发。
这次恢复核验未采集精确上屏耗时，不作为 warm 延迟预算样本。

十万合成 PNG 首屏双 rAF 中位数为 569.6 ms，**未达到 300 ms**；原始构建同目录对照为
554.8 ms，相差约 2.7%。新构建 native 枚举约 257 ms、属性读取约 244 ms，原始构建分别
252／242 ms。没有证据将这个既有目录开销归因于 JPEG 转换；本次也没有宣称修复该预算缺口。

### 交互与启动边界

在本地副本验证了三星评级、四星筛选为空／三星筛选恢复、标签新增／重命名／取消分配、
检查器与 filmstrip 同步、列表／网格切换、真实右键菜单和拖动胶片栏高度 116→194。
使用同名 RAW/JPEG 的共享 sidecar 时，两者评级一致；没有改写 NAS 原照片。

新 profile 首次启动仍有初始 JS 任务（初轮 78–90 ms）和首屏前 GPU 帧间隔。
原始构建对照的首次 contentful paint 为 1156 ms，也有首屏前 643.9 ms 帧间隔。
定位 trace 区分了主线程 Canvas JPEG 解码、同步协议 materialize、默认 DOM 解码与 GPU raster/flush；
前者已移除或改为后台／异步，启动和剩余长帧没有被过滤掉或算作全部解决。

## 验证范围与限制

合成测试覆盖 MPF 关联、越界、截断、角色排除、预算、编辑元数据、ICC、八种 EXIF 方向、
主图比例与小图 contentRect、主图回退、高清派生输入和旧投影恢复。真实相机照片只提供当前样本验证；
不能证明所有相机或被外部软件改写过的内嵌预览均对应主图。

JPEG 规范化支持 8-bit 灰度/RGB 与有效 RGB ICC；CMYK、高位深或超预算输入明确失败。
无法证明的候选不会被标记为 sRGB。完整图像质量策略没有降低；RAW full 先选最大相机 JPEG，
只有长短边均覆盖 RAW 对应尺寸的 90% 时才满足 full，否则继续开发。该 RW2 的最大内嵌 JPEG
当时被 LibRaw 列为 1280×1920，因此最终 full 是 4008×6008 的开发图。
**2026-09-13 修正：这不是文件中实际最大的 JPEG。** 该 RW2 的 IFD0 `0x0127`
另存 6000×4000 的 `JpgFromRaw2`，旧 LibRaw 列表未枚举。现已按标签解析并直接交付，
见 [CR3/RW2 大 JPEG 与交付分析](raw-jpeg-delivery-2026-09-13.md)。
本次不引入 160 像素渐进底图，也不重构完整文件协议为流式传输。

本地 SSD 性能门槛与 NAS 结果分开记录；三轮最大值不是统计稳定的尾延迟。

前端类型检查、211 项测试和生产构建通过；Rust workspace 格式、Clippy 与测试通过，
新增几何、读取预算、旧投影恢复和外部相机 fixture 另做聚焦验证。所有测试应用由 runner 或
验收脚本关闭；照片、私有路径与原始诊断不纳入提交。尚未进行 macOS/Linux 实机验收，
也未将 React 提交次数作为本次性能结论；结论依据真实 WebView 帧、长任务、资源与显示证据。
