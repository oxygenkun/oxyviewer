# Thumbnail 缓存成本研究（2026-09-12）

本报告是优化前的研究记录。后续归因和已完成的生产优化见
[HIF 缩略图优化复测](hif-thumbnail-optimization-2026-09-12.md)。

## 结论

后续整目录验证补充：本报告的 30 次微基准每次使用一个空缓存目录，不能代表缓存里
已有数百个源目录时的成本。后续真实 WebView 的 1,100-HIF 预加载在 150 秒时完成
678 张，后段媒体准备耗时达到约 330–426 ms；当前 `decodeMs` 字段在 Sony 快路径中
还包含此前的缓存准备，不能解释为纯 JPEG 解码耗时。`DiskMediaCache::new` 每次调用
`cleanup_staging` 遍历缓存源目录是明确的扩展性风险，仍需分阶段归因。用户随后选择
保留整个当前文件夹的浏览器缩略图，相关实现及验证见
[Performance Budgets](../PERFORMANCE.md#whole-directory-browser-thumbnails)；Rust 缓存路径未在该实现中改动。

Sony HIF 的内嵌小 JPEG 不应直接套用“解码很贵，所以一律先生成磁盘缓存”的假设。
本次本机热文件页样本里，直接检查并提取内嵌 JPEG 的中位耗时为 2.37 ms，
现有媒体层热磁盘 artifact 查找为 5.81 ms；真正读取缓存 JPEG 只需 0.11 ms。
通用缓存协议的成本明显大于小图片字节读取的成本。

更值得验证的方向是保留源版本绑定的 thumbnail 表示信息：内嵌 JPEG 的 offset/length、
方向、显示尺寸和有效内容区域。一个只用于本研究的定点读取原型，在读取前后检查源版本、
恢复同样 EXIF 方向字段后耗时 0.51 ms，输出与生产提取器的 JPEG 字节逐字节一致。
这支持优先研究轻量表示信息复用，但不等于已经完成生产读取、IPC、缓存失效或 WebView
出图的替代实现。

保留现有磁盘缓存仍有价值：同一生产媒体 API 的冷发布约 19.70 ms，热发布查找约
6.75 ms。对需要真实解码、缩放的格式，以及跨进程重开、慢速源文件存储，不能根据
这个 HIF 样本移除磁盘缓存。方向应是减少小型内嵌缩略图进入通用重路径的次数，
并让视窗附近已准备好的图片直接复用，而不是删除缓存或解除并发限制。

## 测量方法与边界

- Windows、本机文件系统、Rust release build；源为仓库已有 Sony HIF fixture
  `DSC00449.HIF`，9,490,432 bytes。未访问用户的普通应用缓存，未改动源图片。
- 每个实验阶段 30 次；每次冷生成都使用独立临时缓存目录。操作系统文件缓存没有清空，
  初始化 offset 原型还预读了源文件，因此“冷”仅表示媒体 artifact 缓存未命中，
  **不是物理磁盘冷读**。缓存目录是本机临时目录，不是 NAS。
- 中位数取排序后的中间两项均值；P95 使用 nearest-rank，第 29/30 项。
- 直接调用当前 `oxy_media::preview`、`preview_for_app_with_completion` 和资源 registry；
  Sony 检查器及 geometry 模块原样复制进临时例程，文件哈希与生产源一致。
  没有改动生产实现。基准源码、全部样本一并保存。
- 不包含 Tauri `RenderQueue` 的 L1 projection/SQLite 快捷路径、排队、IPC、前端状态更新、
  WebView 解码、GPU 上传及绘制。因此不能把这里的毫秒数当作网格滚动后出图耗时。
- JPEG 解码阶段使用 Rust image 库，**不是 WebView 的 image decode 测量**。
- 各行是各自操作的耗时，不是一个可相加的流水线分解。异步持久化等待尤其不能直接加到
  首次显示成本上，后台写盘可能与前台资源读取重叠。

## 本次数据

| 阶段 | 中位 ms | P95 ms | 说明 |
| --- | ---: | ---: | --- |
| 源版本观察 | 0.30 | 0.45 | 文件身份、尺寸、修改时间及 revision 计算 |
| 读源文件前 256 KiB | 0.17 | 0.27 | 只有打开、读取和关闭 |
| Sony 检查与内嵌 JPEG 提取 | 2.37 | 2.94 | 包含扫描、候选 JPEG 检查、geometry、方向处理 |
| 已知 JPEG 字节范围读取 | 0.08 | 0.16 | 原型，不包含版本校验或解析 |
| 已知表示信息读取，前后各校验源版本 | 0.51 | 0.66 | 原型，输出字节与生产提取结果一致 |
| JPEG 从内存解码 | 0.27 | 0.35 | Rust image 库；不代表浏览器解码耗时 |
| 同步冷生成并持久化 | 30.04 | 38.51 | 非应用兼容 API，返回前完成缓存发布 |
| 同步热磁盘 artifact 查找 | 5.81 | 6.64 | 校验/锁/manifest/lease；不含随后 JPEG 读取 |
| 读取已生成 JPEG 文件 | 0.11 | 0.15 | 只有文件读取 |
| 应用冷生成到资源发布 | 19.70 | 28.22 | 应用媒体 API，持久化异步进行 |
| 应用已发布资源 materialize | 0.01 | 0.01 | 小编码 payload 复制；不含 IPC 和浏览器 |
| 发布后等待持久化剩余时间 | 15.78 | 16.76 | 前台读取 lease 已释放，检查 Persisted 成功 |
| 应用热 publication/artifact 查找 | 6.75 | 8.01 | 已完成持久化；不是 RenderQueue 的 L1 命中 |

前一轮独立的 30 次测量得到提取 2.31 ms、同步热查找 5.66 ms、应用冷发布
19.58 ms（使用上中位数的初步统计），与本轮量级一致。原始数据保留，正式表格
以上述最终运行及统计规则为准。

样本内 JPEG 原始位置为 155,648，长度为 8,294 bytes。生产提取结果额外插入 36 bytes
EXIF 方向字段，共 8,330 bytes，显示尺寸为 120×160。原型只在初始化时确定位置与
方向 recipe；计时部分执行定点读取及版本校验，不重复扫描或 geometry 检测。
该识别方法只作为这个已验证 fixture 的成本实验，不是任意 HIF 的通用 offset 解析器。

## 代码解释

1. [Sony 检查器](../../crates/oxy-media/src/formats/heif/quirks/sony.rs)
   初次读取 256 KiB，必要时扩大到 2 MiB；寻找 JPEG 标记，并调用
   `image::load_from_memory` 检查候选。geometry 检测还会解码选中的 JPEG。
   已知位置、geometry 和方向后，这些重复扫描及检查可以被源版本绑定的信息替代。

2. [DiskMediaCache](../../crates/oxy-media/src/cache/store.rs) 的命中不是一次 `read`：
   它校验源版本，取得进程内锁和跨进程锁，读取 generation，读取 manifest，校验
   artifact，选取满足请求的表示，建立 lease。这些操作对大图缓存必要，但对 8 KiB
   内嵌图片显得相对昂贵。不能为降低开销而直接绕过版本、generation、lease 和发布约束。

3. [应用媒体 dispatcher](../../crates/oxy-media/src/pipeline/dispatcher.rs)
   的 `preview_for_app_with_completion` 使用内存发布和异步持久化；同步 benchmark API
   `preview` 则包含落盘。因此不能拿同步冷缓存 30 ms 解释成用户必须等写盘 30 ms 才出图。

4. [RenderQueue](../../apps/desktop/src-tauri/src/jobs/preview.rs) 在进入上述媒体流水线前
   先查内存 projection 和 SQLite projection。有效的现存 resource 可以直接复用；过期
   resource 才需要恢复及 artifact 验证。这些更快的路径没有在本次微基准里计时。

5. [普通图片分派](../../crates/oxy-media/src/pipeline/dispatcher.rs) 对 JPEG/PNG/WebP
   返回 Original resource，即 thumbnail 也可能读取原图，而不是生成 512 px 的小文件。
   所以 HIF 的结果不能外推给大尺寸 JPEG；后者更应对比原图读取/解码与专用小缩略图。
   RAW、普通 HEIF、TIFF 也需要各自的样本，不能假定都含 8 KiB 的快速表示。

6. [BrowserImageCache](../../apps/desktop/src/lib/browserImageCache.ts) 已保存解码后的图片
   引用，限制为 1,024 项、512 MiB，并最多保留 256 个 native resource lease。
   提前填满整个文件夹的解码缓存可能驱逐视窗附近图片。这个 120×160 样本的 RGBA
   下限为每张 75 KiB，10,000 张约 732 MiB，还没计算浏览器其他开销。

7. 上一轮新增的后台分页只补全文件列表。`backgroundPreviewIntents` 给文件注册排序意图，
   Rust `apply_schedule_changes` 仅调整已存在请求的优先级，不创建任务。普通目录的
   整目录真实 thumbnail 请求尚未接通；本研究没有接通它。

## 是否应提前缓存 thumbnail 信息

建议优先缓存或复用“已识别表示”，而不是再建一个重复的完整 metadata 系统：

- 身份：源 revision、表示/解析策略版本；文件变化后失效。
- 定位：是否有可用内嵌 thumbnail、格式、offset/length；记录确认不存在，避免重复探测。
- 显示：原始尺寸、显示尺寸、方向、内容矩形及 geometry。已有 projection/manifest
  中的字段尽量复用，offset/length 才是当前需要研究的新信息。
- 缓存位置：可以保存稳定的 artifact 身份和位置；不要把进程内 `resourceId` 当作跨启动
  永久标识。重启后必须按现有资源注册机制恢复。

首次识别时保留这些信息几乎不增加源文件 I/O。主动补齐整个目录则仍要读取每个源文件，
因此应首屏后分批进行，优先视窗附近，不能让打开目录等待全目录信息扫描。
对小型内嵌 JPEG，可以进一步研究有界的编码字节缓存；解码图片仍围绕视窗预热。
持久化这份轻量信息是否划算，还要计入批量数据库写入及重启读取成本，本次原型只验证
已经在内存中的 recipe，未实现也未计时其数据库存储。

总成本上，预加载从不免费：没看过的图片也会消耗读取和计算。它的主要收益是把用户
等待移到空闲时间。收益应以首屏是否受影响、视窗内首次/全部出图 P50/P95、实际命中率、
多余读取字节和内存占用来判断，不能只比较整个目录的处理吞吐。

## 现有 UI 证据与下一步决策门槛

本仓库之前的 [filmstrip 测量](filmstrip-scroll-diagnosis-2026-09-11.md) 曾记录某个
旧版本/开发前端样本：提取 33.2 ms，native Ready 到 frontend load 还相隔约 454 ms。
后续 [release A/B](filmstrip-scheduling-2026-09-11.md) 的热返回约 26 ms，冷区域约
242–372 ms。这是旧的底部缩略图条数据，不是本次网格实测；仅说明后端提取与实际
出图必须分开测量，不能承诺把提取从 2.37 ms 降到 0.51 ms 就会解决全部延迟。

生产决策前应做同目录、同滚动轨迹的三组实际 WebView 对照：现状、轻量信息复用、
轻量信息加有界附近解码预热。分别观察首次访问、当前进程返回、重启后返回，以及
本地和实际 NAS 源。完整磁盘缓存保留为基线和慢格式后备，不在本轮决定移除。

## 复现

源码：[probe.rs](thumbnail-cache-cost-2026-09-12/probe.rs)。
最终样本：[measurements.json](thumbnail-cache-cost-2026-09-12/measurements.json)。
前一轮样本：[earlier-measurements.json](thumbnail-cache-cost-2026-09-12/earlier-measurements.json)。

在仓库根目录临时创建 `crates/oxy-media/examples/thumbnail_cache_probe.rs`（复制 probe.rs），
并将生产 `formats/heif/quirks/sony.rs` 和 `formats/heif/quirks/sony/geometry.rs` 原样复制到
该 examples 目录下 `thumbnail_cache_probe/sony.rs` 和 `thumbnail_cache_probe/geometry.rs`。
这是为了在不扩大生产公开 API 的情况下测量私有检查器。随后执行：

```powershell
cargo run --release -p oxy-media --example thumbnail_cache_probe -- tests/fixtures/DSC00449.HIF
```

需要本项目现有 native build dependencies；若 NASM 不在 PATH，设置 `NASM` 为本机编译器
路径。测试完删除上述三个临时 example 文件。不要将临时 parser 副本加入生产源码或提交。
基准在等待异步持久化前释放 protocol read lease，避免阻挡从内存资源到文件资源的转换。
