# 浏览与媒体性能：重构必须保留的约束

本文总结目录浏览、缩略图、媒体缓存和队列诊断已经验证的关键约束。
修改这些路径时，应同时核对 [性能预算](PERFORMANCE.md)、相邻回归测试及目标平台实测。
这是防止回归的设计依据，不是要求永远保留现有实现；替代方案必须证明同样的正确性和交互行为。

## 1. 首屏与后台工作分开

- 打开目录只返回廉价、非递归、分页文件摘要，不同步完成索引、元数据或媒体生成。
- 首屏后才持续取得余下分页；页与页之间让出浏览器渲染机会，不能把后台任务放进一个
  连续同步循环，也不能把“第一页请求成功”当作“首屏已经绘制”。
- 调度 intent 只调整工作顺序，不等于已经提交实际媒体任务。整目录预热必须实际调用
  thumbnail 请求，并确认浏览器完成解码和保留；不能只数 native Ready 或磁盘文件。
- 后台预热保留低优先级和有界在途数量；可见/附近图片继续优先。不要只增加线程数来掩盖锁等待。

## 2. 快路径中不得遍历整个缓存

本次主要退化来自每次 `DiskMediaCache` 构造都遍历 staging，以及每张图完成后立即全库清理。
缓存越大，每个请求越慢；累计工作接近反复执行 `1 + 2 + ... + N`。

- 构造、命中和逐图回复不得扫描其他源目录。启动清理与发布后的容量维护由一个后台 worker
  合并触发；命中缓存不触发清理。当前实现两次维护至少间隔五秒。
- 用共享锁枚举候选；不要在全库遍历期间持有全局独占文件锁。
- 删除之前，必须在短独占区内重新验证 generation、文件状态和最新 lease；只修复涉及的清单。
  候选在枚举后可能被替换、被租用或因 clear 失效，枚举结果不是删除授权。
- 限频扫描仍是 O(N) 后台 I/O，并非零成本或严格实时容量限制。若改成增量计数，要解决重启、
  并发发布、失败写入和跨进程修改的对账问题，不能只加一个会漂移的内存计数器。
- 低优先级不能改变 OS 文件锁的等待顺序；取消令牌也不会自动中断阻塞式取锁。

## 3. 锁只保护状态，不包住慢操作

- 全局 queue mutex 和 projection map 锁内不得执行 SQLite、文件校验、解码或事件发送。
  把同步操作搬到后台线程，并不意味着它持有的全局锁已经便宜。
- 从 map 取值后需要跨 I/O 或重新写 map 时，先取得拥有所有权的快照再释放读锁。
  不要为了省 `.cloned()`，让读锁跨越潜在写锁获取而产生自锁或长时间阻塞。
- 同图 admission 必须串行或显式预留。仅把数据库操作移出全局锁，会让两个请求分配不同
  validAt，随后错误合并到同一个旧任务。当前按 canonical path/level 使用短生命周期 admission gate。
- I/O 之后重新校验 selection、目录、generation 和取消状态；取锁前的检查不能代替提交前检查。
  入队期间即使已经释放 scheduling scope，也必须记住取消。
- 重排 pending 和提升 active 是两件事。正在执行的低优先级工作若需要取消/重启，必须保留
  有效订阅者，并防止旧完成结果覆盖新结果。不要把重启实现成丢失等待者。
- 移出 projection 锁之后，仍要保持 stateRevision/validAt 的顺序约束。旧 descriptor 恢复
  必须做乐观版本校验，不能覆盖同一 validAt 下后来发布的 Ready/Error。

## 4. 缓存命中是一组有效性证明

缓存不是 `path.exists()`。后续精简仍须保留：

| 边界 | 必须证明的事实 |
| --- | --- |
| Projection | 源版本匹配；结果可显示；当前进程资源仍可用或能被安全恢复 |
| 磁盘 artifact | manifest、policy、representation、尺寸/呈现能力、文件完整性和 generation 均有效 |
| 资源恢复 | 持久化 descriptor 不能跨进程复用；验证文件后重新注册并获得 lease |
| 并发生产 | 等待生产锁之后再次查找，复用其他请求已完成的结果 |
| 清理/发布 | clear 推进 generation，旧生产者不能重新填回已清空的缓存 |
| 发布生命周期 | ready、持久化完成、浏览器解码完成是不同阶段；发布和读取所需 lease 必须连续 |

HIF 小 JPEG 的准备成本可能大于真正提取字节的成本。当前把多个候选合并为一次清单读取，
保留生产锁之后的复查。不要恢复被绕过的 ManifestIndex/LRU：文件修改时间不是跨进程一致性令牌。
也不要因为本地单张 Sony HIF 的提取很便宜，就删除其他格式需要的解码和磁盘缓存协议。

## 5. 浏览器保留整个当前文件夹的缩略图

这是明确的产品选择，不应在“统一缓存”重构中被通用 LRU 悄悄改变。

- 当前目录缩略图拥有独立 Blob URL 和已解码 `Image`，不受 full/loupe 的通用图片 LRU 淘汰。
  虚拟列表只减少 DOM 数量，不应顺带驱逐这些保留像素。
- 最长边不超过 512 的已编码小图直接保留 Blob 和解码结果；不要无条件重复执行
  JPEG → Canvas → PNG → Image。较大原图仍须缩小，避免整目录保留全分辨率像素。
- 保留 contentRect 与 displaySize 的区别，验证方向、裁剪和尺寸；缓存命中不能改变可见画面。
- 浏览器副本就绪后释放 native lease，不为所有目录图片长期钉住原生 registry。
- 切目录、显式失效和源变化时释放对应图片与 URL；异步完成必须经过 generation/key 校验。
  单纯清空 map 不足以阻止迟到任务重新写入，也不能回收仍存活的 Blob URL。
- 内存随目录规模增长是该选择的代价。RGBA 估算不含 Blob、浏览器和 GPU 开销；不能把估算写成进程总内存。

## 6. 诊断不能反过来阻塞产品

- 诊断只按需启动；轮询一次最多一条请求。长请求不能被 setInterval 无限制叠加。
- 所有被观察的 queue 和 active request 都必须用非阻塞取锁。只修 preview queue 不够，
  metadata 等队列同样可能正在持锁写数据库。
- 繁忙时保留上次样本并明确标注 stale；第一次尚无样本时不能显示为“空队列”。
- 快照在原生 UI 线程之外执行；分别观察 worker 调度等待、原生采集、IPC 往返和 WebView 渲染。
  一个 RTT 尖峰不能直接归因为解码、数据库或队列锁。

## 7. 防回归验证清单

| 修改范围 | 至少覆盖的验证 |
| --- | --- |
| 清理与缓存查找 | 大缓存规模；并发读取/发布/clear；活跃 lease；旧 generation；损坏 artifact；跨实例清单变化 |
| 入队/投影 | 同图合并；I/O 期间取消；切目录/selection；active 提级保留订阅者；迟到状态与 descriptor 恢复 |
| 诊断 | 人为持有 queue 和 active request 锁时立即返回；保留旧样本并标记 stale；无重叠轮询 |
| 浏览器保留 | 超过通用 LRU 数量；虚拟卸载/重挂；源变化与晚到 fetch；大图缩放及 geometry；URL 释放 |
| 最终交互 | 正式构建的真实 WebView；冷目录预热、热滚动、胶片栏、冷预览；原生资源预算与实际可见画面 |

相邻测试包含 `under_budget_prune_does_not_block_on_cache_readers_or_publication_mutex`、
`candidate_lookup_preserves_order_policy_identity_and_miss_generation`、
`cancellation_during_admission_is_retained_after_scope_release`、
`stale_descriptor_restore_cannot_overwrite_a_newer_state_of_the_same_request`、
`snapshot_never_waits_for_queue_or_active_request_locks`、
`busy_snapshot_keeps_the_last_sample_and_marks_it_stale`，以及 `folderThumbnailCache.test.tsx`。

不要只用空缓存微基准、构建成功或 mock 浏览器证明性能。使用增长中的真实缓存，分别报告首屏、
单张 native、整目录浏览器预热、热滚动 readiness 和内存；并标注是否清除了 OS 文件缓存。
测试数据与用户状态隔离，完成后关闭测试实例；公开测量记录去除机器和 NAS 路径。

## 已知结果与未完成预算

2026-09-12 的最终 Windows Release/WebView2 测试：同一 HIF 夹具的 1,100 个路径在 29.3s
全部保留；热滚动新增原生请求为零。快照原生采集最大 34μs，完整往返 P95 为 4ms。
这不是不同照片冷盘/NAS 的性能保证，也不是零 IPC 尖峰保证。

**首屏仍未稳定达到 300ms 目标**：最终 HIF/PNG/胶片栏样本分别为 361.7/316.2/384.8ms。
吞吐提升不能被表述为首屏预算已经解决。后续应独立分析摘要返回后的 WebView 渲染，
不要通过放宽预算或删除观测点掩盖剩余延迟。

详细证据见 [优化与复测](research/hif-thumbnail-optimization-2026-09-12.md) 和
[优化前的锁竞争分析](research/hif-thumbnail-contention-2026-09-12/README.md)。

## Background analyzers

Face analysis has one admitted job at a time, independent of the image worker
count. It waits on the cancellable foreground gate before each asset and asks
for Full at Preload priority. Folder opening never starts model inference.
Targets use 256-item keyset pages with a persisted index generation and cursor;
per-asset checkpoints survive interrupted pages. Expensive clustering/matching
loops check cancellation internally and publish their derived tables together.
The first-party worker has cooperative cancellation plus bounded termination;
shutdown uses the shared JobRegistry. See the [local qualification](research/face-analyzer-host-2026-09-19.md).
