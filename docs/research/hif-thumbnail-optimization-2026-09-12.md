# HIF 缩略图优化与桌面复测

日期：2026-09-12。已实施生产修改，正常保留持久化和容量管理，无诊断旁路开关。

## 已完成的四步

1. **缓存维护移出逐图路径。** `DiskMediaCache` 构造不再遍历 staging。
   启动和发布完成由 `CacheManager::schedule_prune` 合并到一个后台 worker，
   两次维护至少间隔五秒；命中缓存不触发维护。全库枚举只持共享文件锁，删除时
   才按候选逐个持独占锁，并复查 generation、文件大小/修改时间、当前租约。
   只修复该候选所属清单；未超容量时也会回收过期且无租约的 staging。
2. **缩短队列和 projection 锁。** 入队按 canonical path/level 串行，SQLite、
   文件校验及事件发送移出全局 queue mutex。入队和回复前复查 selection、目录、
   cancellation 和 generation；保留 active 提级时的订阅者。Projection map 不再
   持锁写 SQLite；恢复旧 descriptor 时通过 stateRevision 乐观校验，防止覆盖
   同一 validAt 下已经发布的新状态。原生快照在 blocking worker 执行，对所有
   queue/active request 使用 try_lock；繁忙时保留上次样本并列入 staleQueues。
   面板一次最多一个查询，不会因观察其他队列而等待它们的数据库事务。
3. **合并 HIF 候选查询。** 普通预览和 Sony embedded policy 以一个清单读取完成
   检查，生产锁之后再执行一次完整复查，冷 embedded 路径从五次清单查找减为两次。
   移除一直被绕过的 ManifestIndex/LRU；保留源版本、artifact 完整性、lease、
   generation、active publication 及完整解码 fallback。
4. **浏览器直接保留小图片。** 最长边不超过 512 的图片直接持有原始 Blob URL 和
   已解码 `Image`，省去 Canvas → PNG → 第二次解码。更大原图继续缩小至 512，
   缩放 contentRect 并保留 displaySize。整个当前目录继续保留，切目录/源变化
   继续释放或失效，浏览器副本不长期占用 native lease。

主要实现：

- `crates/oxy-media/src/cache/v2.rs`
- `apps/desktop/src-tauri/src/state/cache.rs`
- `apps/desktop/src-tauri/src/jobs/preview.rs`
- `crates/oxy-library/src/lib.rs`
- `crates/oxy-media/src/pipeline/heif/artifact.rs`
- `apps/desktop/src/lib/cache/folderThumbnailCache.ts`

## 缩略图主路径复测

使用真实 Release/WebView2 和 Rust 媒体路径；同一 `DSC00449.HIF` 以
hardlink-or-copy 生成 1,100 个路径。每轮清空 runner 的隔离应用状态与缓存，
没有清空 OS 文件缓存。测量期间无并行编译。

| 指标 | 旧基线 | 优化轮 1 | 优化轮 2 |
| --- | ---: | ---: | ---: |
| 整目录浏览器预热 | 150s 时仅 678/1100 | 31.25s，1100/1100 | 29.24s，1100/1100 |
| 末段 20 张 native total 中位数 | 177ms（旧 30s 诊断轮） | 9ms | 9ms |
| Queue 查询中位数 / P95 | 旧报告只列最大 RTT | 0.9 / 4.3ms | 0.9 / 4.2ms |
| Queue 最大 RTT | 214.2ms（旧 30s 诊断轮） | 190.9ms | 304.3ms |
| 首屏开始渲染 | 不用于本次速度对照 | 177.9ms | 302.3ms |
| 热滚动新增 native 请求 | — | 0 | 0 |
| 四次热滚动可见图片 ready | — | 67–144ms | 64–123ms |

两个旧基线来自此前不同长度的诊断轮，不能合并成同一条时间序列。
上述两轮在最后补充非阻塞快照之前完成，显示持续缩略图准备明显加快；
不声称所有首屏/IPC 尖峰都已消失。
首屏第二轮 302.3ms 略高于 300ms 目标；两轮中位数为 240.1ms，本场景没有配置
该项预算门禁。热滚动检查包含至少 40ms 的等待，是整个可见视窗 readiness，
不能解释为精确的首像素延迟。

1,100 张保留缩略图的 RGBA 估算均为 84,480,000 字节（约 80.6 MiB），
不含 Blob 和浏览器开销。原生 registry 均峰值 77/512，结束时 1 个 entry，
UI/read lease 均为 0。没有使用扩大 native 资源预算来获得吞吐提升。

数据：[两轮脱敏样本](hif-thumbnail-optimization-2026-09-12/measurements.json)。
对照：[优化前锁竞争报告](hif-thumbnail-contention-2026-09-12/README.md)。

一次追加分段计时发现，原生采集虽然中位数仅 10μs，但仍有 162.64ms 的等锁尖峰；
该轮预热完成为 26.49s，首屏为 881.4ms。仅把命令移到 blocking worker 能避免
直接阻塞原生 UI 线程，却不能保证观察者自身快速返回，因此最终加入上述 try_lock
和过期样本机制。该中间轮保留在
[阻塞快照计时](hif-thumbnail-optimization-2026-09-12/blocking-snapshot-timing.json)。

## 最终非阻塞快照与整目录验收

最后的 Release 构建再次完成 1,100/1,100 HIF，预热 **29.30s**，末段 native total
中位数 **8ms**。四次热滚动检查为 58.3/63.1/94.6/72.0ms，新增 native 请求为 0。
保留 RGBA 估算仍为 80.6 MiB；native registry 峰值 68/512，结束时 UI/read lease 为 0。

115 次快照的分段统计：

| 耗时 | 中位数 | P95 | 最大值 |
| --- | ---: | ---: | ---: |
| 原生快照采集 | 18μs | 24μs | **34μs** |
| 原生 blocking worker 调度等待 | 16μs | 25μs | 46μs |
| WebView 观察到的完整 RTT | 0.8ms | 4.0ms | 87.4ms |

两次快照遇到了繁忙的 metadata 队列，分别在 21/24μs 内返回，并设置 staleQueues。
这验证了真实争用时采用旧样本，而不是依赖恰好没有争用。最大 RTT 的原生采集仅
21μs，所以仍有的端到端延迟位于采集之外；本报告不把它归因为已消除的队列等锁。

本轮首屏 361.7ms（第一页摘要返回 27ms），仍超过 300ms 目标。持续预热和观察者
锁等待已经修复，首屏渲染的剩余延迟没有被宣称达标。
最终样本：[非阻塞快照计时](hif-thumbnail-optimization-2026-09-12/nonblocking-snapshot-timing.json)。

## 验证与复现

- 前端类型检查、200 项测试、生产构建通过。
- Rust workspace 测试通过；之后补跑了修改涉及的 library/preview 测试，
  以及候选查询、入队取消和同请求旧状态恢复回归测试。workspace Clippy 和 fmt 通过。
- 正式 `pnpm tauri build --no-bundle` 构建通过，包含正常 bundled FFmpeg。
- 冷 HIF 首预览 261.4ms，满足 800ms 门禁；首 tile 881ms。
- 最终构建的 HIF filmstrip-scroll 场景完成，三个可见视窗 readiness 为
  25.2/26.7/25.8ms。PNG 原图资源也完成 1,100/1,100，预热 19.22s，四次热滚动
  仍为零新增原生请求。两场景首屏分别为 384.8/316.2ms，同样不算首屏预算达标。
  [补充检查](hif-thumbnail-optimization-2026-09-12/additional-checks.json)。

```powershell
pnpm tauri build --no-bundle
node scripts/perf/perf-e2e.mjs --scenario folder-thumbnail-retention-hif --runs 2 --verbose
node scripts/perf/perf-e2e.mjs --scenario cold-preview-hif --runs 1 --verbose
```

`hif-thumbnail-optimization-2026-09-12/analyze.mjs` 接受一个或多个 runner report，
最后一个参数是输出 JSON；只保留无机器路径的数值和公开夹具信息。

## 当前边界

维护仍是限频的 O(N) 后台扫描，没有增加易漂移的增量容量账本。磁盘很大或明显
超容量时，维护仍需要 I/O；共享枚举可以与读取/发布并行，但 clear 仍等待共享锁。
缩略图 preloader 保持单请求在途，保留 visible/nearby 的抢占空间。此轮没有引入
新的源文件 offset/recipe 缓存，也没有取消既有磁盘 artifact 协议。

结果只覆盖本地单张 HIF 的多路径压力，不能外推为 1,100 张不同照片的冷盘/NAS 性能。

所有 runner 启动的测试实例已关闭。重构约束集中记录于 [性能不变量](../PERFORMANCE_INVARIANTS.md)，并由仓库 AGENTS.md 引导后续修改者阅读。
