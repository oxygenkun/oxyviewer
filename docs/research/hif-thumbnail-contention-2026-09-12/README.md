# HIF 缩略图耗时与 queue 锁竞争分析

日期：2026-09-12。性质：诊断与方案分析；临时插桩和跳过维护的开关已从生产源码撤回。

本报告描述优化前的基线；下文的“当前实现”和源码行号均指诊断时状态。
后续已实施的维护、入队、HIF 候选查询和浏览器 Blob 优化，见
[实现与复测](../hif-thumbnail-optimization-2026-09-12.md)。

## 结论

当前 HIF 小缩略图路径的主要瓶颈是逐张触发的全缓存目录扫描和扫描持有的全局文件锁。
queue 面板另有独立问题：请求入队持 queue 锁进行 SQLite 事务，而面板的同步命令在原生
main 线程等待同一把锁。两条链都已经实测，不能把全部耗时解释为 JPEG 解码。

移除两处扫描的诊断对照显著提高预热吞吐，但没有消除首屏 queue 卡顿。因此仅增加线程、
调高缩略图优先级、缓存更多浏览器图片，均不能解决这两个后端问题。

## 测量边界

- 使用当前工作区的 Release 桌面构建、真实 WebView2、实际 Rust 媒体路径。
- 桌面测试使用同一真实 `DSC00449.HIF` 经 hardlink-or-copy 生成的 1,100 个路径。
  每次调用性能 runner 都清空该场景的隔离应用状态；未清空操作系统文件缓存。
  结果不代表 1,100 张不同照片的冷盘或 NAS 读取。
- 每轮观察 30 秒，以浏览器已完成解码并持有的缩略图数衡量进度，不把 native Ready
  或磁盘 persisted 数量当成浏览器完成数。未把未完成的 1,100 张测试报告为全量成功。
- 采样器每 250ms 调用实际 `get_debug_queue_snapshot`，最多一条诊断查询在途。
  原有 dashboard 没有这个防重叠限制，所以本次未人为制造它的重复请求积压。
- Native 插桩记录构造、维护、generation 文件锁等待、入队持锁时间和 snapshot 锁等待。
  generation 日志只记录超过 2ms 的等待，因此其统计是慢样本条件分布。
- 各线程时间会重叠，日志中的 duration sum 不能相加当作应用总运行时间。
- 每个桌面对照只有一轮，数量用于因果定位，不构成稳定性能预算。

## 桌面 A/B 结果

| 条件 | 30 秒浏览器已保留 | 最后 20 次缩略图 native total 中位数 | queue 查询最大 RTT |
| --- | ---: | ---: | ---: |
| 当前实现 | 297 | 177ms | 214.2ms |
| 仅跳过 constructor 的 cleanup_staging | 349 | 169ms | 300.3ms |
| 仅跳过 prune_with_protected | 374 | 119ms | 152.0ms |
| 两者均跳过 | 908 | 14ms | 195.7ms |

“仅跳过 prune”仍会先执行 `CacheManager::prune_after_write` 中构造 cache 的扫描。
“两者均跳过”仍执行原本的 SQLite、请求队列、资源发布、JPEG 落盘持久化、IPC、
WebView 加载、Canvas → PNG Blob 和解码保留路径。它不等于禁用持久化。
这些开关只用于诊断，跳过容量管理不能作为生产修复提交。

正常轮末段的独立统计：

- 构造 cache 中位数 75.14ms。
- 一次写后 prune 中位数 123.10ms。
- generation 慢等待中位数 95.60ms。
- 单张 native total 中位数 177ms。

构造耗时与 generation 等待已能解释该段大部分延迟。后台 prune 的 123ms 与请求等待
存在重叠，不能再次完整加到 177ms 上。

原始脱敏样本见 `desktop-baseline.json`、`desktop-skip-constructor-scan.json`、
`desktop-skip-prune.json`、`desktop-skip-both-scans.json`。

## 1. 每张图扫描全库，形成随目录增长的重复工作

调用链：

```text
preview_for_app_with_completion
  -> HEIF render_preview
  -> ArtifactCache::new / for_source_revision
  -> DiskMediaCache::new / with_lease_ttl
  -> cleanup_staging
     -> 遍历所有分片目录、所有 source 目录、各 source 的 .tmp
```

位置：`crates/oxy-media/src/pipeline/artifact.rs:202`、
`crates/oxy-media/src/cache/v2.rs:319`、`:1337`。

该操作不是只检查当前图片。即使所有临时目录都为空，也要逐个查询和遍历。
从空缓存生成 N 张图，构造路径的总扫描规模趋向 `1 + 2 + ... + N`。
它会受整个缓存规模影响，不仅是当前文件夹。

持久化完成后的 `CacheManager::prune_after_write` 又构造一次 cache，重复扫描。
重启恢复 projection 的 `restore_projection_resource` 也构造 cache。
缓存本来应该加速后续请求，当前这条构造路径却让缓存越大，单次请求准备越慢。

独立扫描测试使用同样的分片/source/.tmp/小文件布局，但不做图片解码：

| source 数量 | constructor 中位数 | 未超容量的 prune 中位数 |
| --- | ---: | ---: |
| 0 | 1.50ms | 0.36ms |
| 100 | 28.72ms | 47.79ms |
| 300 | 75.97ms | 149.18ms |
| 625 | 175.16ms | 314.09ms |
| 1,100 | 284.63ms | 572.55ms |

这组是 scan-only 合成目录，具体数值不应冒充桌面真实缓存扫描时长。
线性增长关系与实际应用插桩一致，原始样本见 `cache-growth.json`。

## 2. 低优先级维护拿全局独占锁，挡住前台读取

`DiskMediaCache::prune_with_protected` 在 `v2.rs:683` 获取 `.cache.lock` 的独占锁，
然后 `collect_artifacts` 全库遍历，统计完成后才能知道是否超容量。
当前已有“未超容量提前返回”，但返回点在全库扫描之后。

`generation()` 和 `lookup_or_generation()` 需要同一个文件的共享锁。
`ArtifactPublisher::lookup_active` 甚至在检查进程内 active publication 前，
先调用 `cache.generation()`。所以内存中已有资源的查找也可能等待全库清理。

独立实验使用两个不同的 `DiskMediaCache` 对象，排除了同一 Rust operation Mutex 的影响：

- 无清理时，generation 读取约 0.3ms。
- 1,100 条目并发清理时，同一读取等待 465–633ms。
- 625 条目并发清理时，等待 292–346ms。

这是实际的跨对象文件锁竞争。调度器的 priority/rank 不会让 OS 文件锁继承前台优先级。
前台已拿到 worker，也可能停在这里；取消令牌同样不能中断正在进行的阻塞式文件锁等待。

写后清理还有两个入口：`jobs/preview.rs` 的异步 persistence completion，以及
`commands/preview.rs:55` 对返回结果 `Persisted` 的检查。后者也可能为缓存命中触发清理，
并不只发生在生成新文件之后。`prune_state` 合并了同时发生的触发，但未限制每轮的扫描规模
或把完成一张→清理一轮的频率降下来。

## 3. queue 面板确实等待了业务锁，而且等待发生在 main 线程

入队路径 `apps/desktop/src-tauri/src/jobs/preview.rs`：

```text
request:
  work Mutex                         # line 711，直到入队完成才释放
    -> library.next_resource_revision  # line 800，SQLite 写事务
    -> transition                      # line 809
       -> projections RwLock(write)    # line 1133
       -> library.accept_image_projection
          -> SQLite writer Mutex + IMMEDIATE transaction
    -> app.emit
    -> pending.push
```

`debug_snapshot` 在 line 1076 获取同一个 work Mutex，再读取 active request Mutex。
`commands/system.rs:34` 是同步 Tauri command；实际日志显示执行线程为 `Some("main")`。

正常桌面轮测到：

- 299 次入队的 queue 持锁时间：中位数 4.41ms，P95 12.57ms，最大 43.89ms。
- 其中 revision 写事务中位数 2.03ms，Loading transition 中位数 2.04ms。
- 入队等待 queue 锁：P95 200.57ms，最大 310.20ms。
- snapshot 等缩略图 work 锁：168.30ms、202.94ms；对应前端 RTT 170.1ms、214.2ms。
- 第二个慢样本时，缩略图 active=24，pending=35；metadata 和 libraryIndex 都是空队列。

这不需要一次 SQL 持锁 200ms：一批请求各占锁几毫秒，加上无严格公平保证的 Mutex
连续竞争，就会让后来的请求与 main 线程等待数百毫秒。
`image_worker_count` 当前等于可用逻辑 CPU 数，本机实际活跃到 24 个 worker。
增加 worker 不会增加 SQLite writer 的并行度，可能使争用更集中。

元数据入队 `jobs/metadata.rs:208` 也在持 work 锁期间调用 revision 分配和
`accept_metadata_projection`。它是同类风险，但这轮 queue 慢样本不是由活跃元数据工作解释的。
`transition` 和恢复资源后重新接受 projection 也会持 projection 写锁执行 SQL。

原 dashboard 每 250ms `setInterval(refresh)`，没有 in-flight 防重叠；若等待超过间隔，
会继续增加诊断查询。一次 snapshot 依次取多个队列，任一锁慢都会延迟整个结果。
因此观察面板并不满足注释所说的“cheap point-in-time view”。

没有在这次实验中观察到永久死锁，也没有在已审查的 work→active / work→projection→DB
路径找到必然的反向持锁闭环；可确认的是锁竞争、队头阻塞、优先级无法传递到维护锁。
“未发现永久死锁”不是对所有运行路径无死锁的保证。

## 4. 去掉扫描后仍有多层开销，不能把 2.35 秒当成界面完成时间

独立 native 分段测试：31 个 fixture 路径、关闭 constructor 扫描、每次完成后等待后台
持久化；下表排除首次 publisher 初始化，统计后 30 次的均值。
该测试没有 SQLite、Tauri 或 WebView，也没有并发清理。

| 阶段 | 平均耗时 |
| --- | ---: |
| SourceRevision / cache 构造 / publisher 获取 | 2.28ms |
| 首轮完整/嵌入候选缓存查找 | 2.55ms |
| prepare 再检查 | 1.05ms |
| 获取同源 fast lock 后再查完整/嵌入候选 | 2.05ms |
| Sony inspect（读头、找 JPEG、校验、方向/几何） | 1.32ms |
| 发布 encoded resource | 0.69ms |
| HEIF 路径合计 | 9.95ms |

这次小样本的提取时间与此前 1,100 次长循环的 2.14ms/张不是同一轮数据，不能混用相减。
当前冷路径需要最多五次 `lookup_or_generation`：首次两个候选、prepare 一次、
同源锁后再两个候选。每次可能重新观察 source、打开 generation/source 锁、读取 manifest。
重复验证应通过统一候选选择与明确的再次验证点合并，而不是删掉所有一致性检查。

资源读取/物化小 JPEG 中位数仅 0.0083ms；本轮没有发现大块图像字节复制的瓶颈。
另外等待后台 JPEG/manifest 落盘完成的剩余时间中位数 18.41ms，与显示发布时间分开计量。
生产代码中后台持久化只有一个 worker，完成通知又逐张启动线程并触发维护。

前端目前逐张串行等待完整链路：native result → renew IPC → 原 JPEG Image 加载/解码 →
Canvas → PNG Blob 编码 → PNG Image 再解码 → 保留，再开始下一张。
这使 native 小图提取、通信和前端转换无法充分重叠。没有把这部分未单独插桩的耗时
精确归因成 PNG 编码；它是代码确认的额外工作，不是这次全库扫描主因的替代解释。

## 5. 指标和复杂度上的额外问题

- `pipeline/heif/artifact.rs:178` 的计时器在构造 cache 前开始，line 226 将整个此前耗时
  写入 `decodeMs`；它包含扫描、锁等待和缓存验证，`queueWaitMs` / `sourceWaitMs` 却写 0。
  因此原本看到的 400ms decode 不能作为真实 JPEG 解码耗时。
- `ManifestIndex` 仍维护缓存/LRU，但生产的三个 `load_manifest` 调用都传
  `allow_cached=false`。保留索引写入和复杂度，却没有这条命中路径的收益。
  这不是本次最大耗时，但应该清理或明确用途。
- 资源 registry 的 `resolve` 在持全局 state Mutex 时观察文件 revision；`renew` 在锁内
  更新 lease marker，过期资源析构也可能清理文件。属于应缩短的 I/O 临界区，
  但本轮没有独立证据将 200ms 卡顿归因给它，不能与已实测的 queue/cache 锁混为一谈。
- shared publisher 初始化会持全局 map Mutex 创建 cache，首个大缓存初始化也可能扩大等待。
  稳态热点仍是每请求新建的 ArtifactCache/DiskMediaCache，以及每次 prune 的新建。
- Native queue 只描述 pending/active 请求，不展示申请入队前的验证、后台持久化和全库清理。
  `Ready`、浏览器已解码、磁盘已持久化是三个不同时间点。窗口里的 queue 状态不足以独立定位。

## 建议的处理顺序

1. **将维护移出单张请求热路径。** cache 生命周期集中管理；临时文件清扫做一次性或低频后台
   工作。按增量维护使用量，周期性校准；写后触发合并/节流。避免拿全局独占锁遍历全库，
   删除时再做短临界区的 generation 与 lease 重检。仅复用 cache 对象不能解决 prune 全局锁。
2. **缩短 queue 临界区。** 锁内只处理队列、订阅者和调度状态；数据库和 emit 移出，重新入锁后
   重检 selection、generation、同 key 工作和 revision。保留现有过期结果隔离语义。
   snapshot 使用非阻塞读取/最近快照，主线程不等业务锁；前端完成上次请求后再刷新。
3. **为便宜的嵌入缩略图收窄路径。** 合并候选缓存查询；评估直接提供嵌入 JPEG 的 encoded
   resource/source recipe，而不是每张都套完整持久化 artifact 生命周期。
   保留 source revision、取消、方向/裁切与资源有效性。原生协议继续传二进制，不通过 JSON IPC。
4. **减少前端转换并适度流水执行。** 已经小于 512px 的嵌入 JPEG 可直接保留 JPEG Blob，
   避免 Canvas→PNG→再解码；大图仍需要缩小。随后再评估小并发流水线，不直接增加大量 worker。
5. **明确瞬时状态是否需要持久化。** 先解决锁内 SQL；再评估 Loading/排队状态是否必须跨进程
   持久化。若改变 projection/修订号契约，需单独设计恢复和冲突语义，不应简单删表或去锁。
6. **补充可信指标。** 分开 admission wait、queue wait、source extraction、cache constructor、
   manifest lookup、file-lock wait、SQL、persistence 和 WebView 完成。展示维护状态与查询耗时。

安全文件发布、source revision、租约、取消及全尺寸解码调度都有必要。当前复杂度的问题是
全库维护、每图 SQL 和重复查找进入了毫秒级小缩略图的热路径，且观察者也会阻塞主线程。

## 复现文件与收尾

- `cache-growth-probe.rs`：临时放到 `crates/oxy-media/examples/hif_cache_contention.rs`，执行
  `cargo run --release -p oxy-media --example hif_cache_contention -- .tmp`。
- `diagnostic-only.patch`：基于当时工作区的临时插桩，仅适合隔离诊断，不是生产修复。
  包含在显式 perf 场景开放 queue 查询和两个维护旁路环境变量。
- `native-phases-probe.rs`：配合插桩临时作为 `hif_pipeline_phases` example；参数是包含
  HIF fixture 的目录。设置 `OXY_PROFILE_SKIP_CONSTRUCTOR_SCAN=1` 后采集 stdout/stderr，
  交给 `analyze-phases.mjs results.json stderr.log output.json`。
- 桌面场景使用 `resourceStress: "folder-thumbnails"`、`enterLoupe: false`、
  `awaitMarks: ["resource:stress-complete"]`、`coldCache: true`、`timeoutMs: 60000`。
  fixture 是 `tests/fixtures/DSC00449.HIF`，count=1100。runner 的 stderr 保留上限临时提高到 4MB。
  `OXY_PROFILE_SKIP_CONSTRUCTOR_SCAN=1` / `OXY_PROFILE_SKIP_PRUNE=1` 仅分别用于上述对照。
- `analyze-desktop.mjs report.json output.json` 读取相邻的 `.stderr.log`，输出去除本地路径的样本。
- 源码共 7 个临时改动文件均以插桩前 SHA-256 验证恢复；临时 example 从 crate 移除。
  原有未提交修改保留。测试实例由 runner 关闭；正常桌面版本在恢复后重新构建。
