# 内嵌元数据批量写回优化验收 — 2026-10-16

后续：[有界并发验证](2026-10-16-embedded-metadata-concurrency-validation.md)比较 1/2/4 路，
将默认 writer 数改为 2。本文保留此前单会话实现及测量。

## 实现

落实[诊断报告](2026-10-15-embedded-metadata-write-diagnosis.md)的第一阶段：

- 文件夹只发一次 IPC，MetadataService 登记一个 batch job，由 CoreRuntime
  拥有 worker。一个批次惰性创建一个 ExifTool stay-open session，保持单文件在途。
- 写前通过 media 的 NativeMetadataReader 读取真实内嵌字段，与侧车比较评分、
  颜色、旗标及两类关键词。相同则计为 unchanged，不启动 writer、不修改原图。
  不使用叠加侧车的应用详情，也不凭保留的 mtime 或大小缓存“已同步”结论。
- 非空关键词用重复赋值替换完整列表，空列表显式清空，修复先清除再追加造成的
  重复值。parser 保留 XMP 数组元素，media 不按显示逗号拆分真实数组。
- 元数据按路径失效，详情缓存每路径保留一份带版本证明的结果，通过路径直接删除，
  不为逐文件写入遍历整个详情缓存。保留其他文件的详情和请求 generation。旧详情等待者重新
  观察后入队；全局 queue/map 锁不跨文件或 SQLite 操作。写后摘要刷新以 16 个
  路径或 250 ms 为 admission 边界、使用 Background 优先级。
- 小进度事件带 requestId/jobId；最终命令结果包含权威计数和逐文件失败详情。
  前端区分写入、未变化、失败、跳过，保留取消、提供者重新检测和失败后的部分成功。

session 同时排空 stdout/stderr，每次同时等待编号 ready 和退出状态。普通文件错误
可继续，丢失确认则标记 uncertain、停止后续文件，不自动重启或重放。换行、非 Unicode
及 argfile 无法忠实表示的首尾空白参数使用 argv 传输。提供者切换后下一文件重建
session。取消停止后续文件；当前调用完成后才释放 worker。关闭 deadline 超时返回
未 join，测试不把正在执行的 native 操作报告为已终止。

## Release 测量

macOS arm64，ExifTool 13.55（显式配置的本机程序；托管版本仍固定 13.59）。使用与
前次相同的三张真实 Sony HIF，大小 7,773,136、10,031,104、10,186,752 字节。
原图和原 XMP 只读；每次把独立副本放在本地临时目录或原 SMB 目录的新隐藏临时
子目录，绝不使用硬链接。完成后自动删除所有测试副本。

可复用 harness：
[embedded_sync_latency.rs](../../crates/oxy-core/examples/embedded_sync_latency.rs)。
先以 release 构建 example；参数是 scratch parent、ExifTool、ffprobe 和源 HIF 列表。
三轮各建立两组全新副本，交替两种方案的先后顺序；每组三张真实变更后立即重复相同
内容。没有清 OS/NAS 缓存，没有控制服务端负载。最终采样没有同时运行编译器。

基线重建原先逐张启动 ExifTool 的 writer 参数，**不含**旧版 sidecar 读取、IPC、job
登记和投影成本；新方案计时完整公开 core batch 至回复，包括比较、session 启动/
关闭、失效及摘要刷新准入，不等待所有异步摘要完成。此表是核心/传输对照，不能
当作完整旧/新 WebView 交互耗时对照。

| 位置与输入 | 原 writer 九文件总计 ms | 新 core 九文件总计 ms | 总耗时减少 |
| --- | ---: | ---: | ---: |
| 本地，真实变更 | 1,506.726 | 600.557 | 60.1% |
| 本地，相同内容 | 1,499.704 | 9.973 | 99.3% |
| SMB，真实变更 | 3,470.421 | 2,709.105 | 21.9% |
| SMB，相同内容 | 2,689.587 | 130.886 | 95.1% |

三张一批的新方案变更耗时：本地 197.312–202.025 ms，SMB 898.908–906.820 ms；
相同内容分别为 3.299–3.340 ms、42.909–44.942 ms。每轮批次都会关闭 session，
没有剔除首次写入模块加载的成本。逐轮原始数据见
[results.json](2026-10-16-embedded-metadata-sync-results.json)。
精确详情缓存从遍历改为路径删除之前，另一轮 SMB 对照同样得到真实变更缩短 21.7%、
重复同步缩短 95.3%，原始数据一并保留；两轮都仅为小样本。

全部新方案变更均验证五类可编辑字段，重复检查均为 updated=0、unchanged=3，
文件 SHA-256 和修改时间不变。所有新旧测试副本的 ffprobe 压缩图像 packet 数量、
stream index、size 和 SHA-256 与源相同；三份源 HIF 和原 XMP 的 SHA-256 前后相同。
这证明此样本的压缩图像负载未改变，不代表所有容器项、跨平台属性或断电耐久性认证。

## 实际界面和回归

通过仓库 wrapper 构建 macOS Release `.app`，用独立 core-v2 数据/缓存启动，
在真实 WKWebView 中从文件夹右键执行三张本地 HIF 副本：

1. 首次显示“写入 3，未变化 0，失败 0，跳过 0”。执行中观察到 2/3 进度。
2. 再次显示“写入 0，未变化 3，失败 0，跳过 0”。
3. 只把第一份侧车的评分从 5 改为 4，显示“写入 1，未变化 2，失败 0，跳过 0”。

构建后的 browser demo 同样验证首次写入一张 JPEG，以及重复操作未变化一张；
另外九张不具备侧车的照片跳过。这里的可见验证使用本地副本；SMB 表来自 release
core harness，没有测量整批操作的 WebView 延迟分布或写入期间 NAS 浏览交互。

回归覆盖 JPEG/HIF 真实 ExifTool 往返、多项/清空/Unicode/逗号关键词、外部原图
修改后的再次同步、换行文件名、错误后继续、丢失确认不重放、去重/跳过、按路径失效、
旧等待者重入队、取消及 active native shutdown。前端覆盖分页后一次提交、订阅先于
入队、提供者重新检测续传、权威最终结果、取消早到/迟到事件、卸载和丢失回复不重试。

通过 `pnpm check`、`pnpm test`（444 项）、Release wrapper 的前端 build、
contracts generate/check、fmt、workspace Clippy（含 contracts）与 workspace 测试。
第一次整套运行中一个未改动的 people preparation 用例失败，单独重跑及再次完整
workspace 运行均通过。测试结束关闭自启的应用和预览服务。

## 限制和后续

SMB 上真实变更仍会重写整个 HIF，收益受 NAS 负载影响。此前已有数秒一张的波动，
本次三文件小批次不能推算完整 146 张的耗时，不能承诺所有 NAS 都缩短相同比例。
未增加并发，没有更改 native 依赖，也没有接入本地暂存上传替换。后者仍需要在 files
实现逐文件来源证明、恢复、属性保留与故障测试，不能直接采用前次性能原型。
