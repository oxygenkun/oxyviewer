# NAS HIF grid：停滚后补图延迟（2026-09-09）

在用户提供的 SMB 目录复现了停滚后整屏补图慢，并在同一目录完成改动前后三轮对照。
当前案例不需要增加 preview worker 或再增加调度层；主要收益来自减少每张图片的网络读取，
同时解除前台缓存查询与后台持久化之间不必要的锁等待。

## 案例和测量方法

目录：`/Volumes/photo/2026/20260607 新歌空间 7.in ShangHai Fes Vol.1 I see the light序章~RealizE初披露/05 心跳序曲Prologue`，383 个条目。

- macOS，当前工作区构建的 release 应用，真实 Tauri / WKWebView / SMB 路径。
- `grid-scroll` 使用竖图 grid，关闭 inspector；每段连续发送 32 次滚动，间隔 16 ms，
  依次停在滚动范围的 45%、100%、20%。没有在每次滚动之间等待 160 ms 的 idle gate。
- 从最后一次滚动后的停止标记开始，每 25 ms 检查视口。只有所有可见卡片都持有已加载的
  displayed `<img>` 才记录整屏 ready；隐藏的 pending image 不计入完成。
- 每轮使用隔离的应用数据和冷 artifact cache。未清除操作系统或 NAS 服务端缓存，
  所以这里的 cold 指应用缓存，不是物理冷盘。源照片没有修改。
- 修改前第三轮有其他本地构建/测试活动；这些数据用于定位问题和确认改善趋势，不作为
  严格隔离 CPU 的正式基线。额外的重命名 executable 对照因窗口未激活、WebView 定时器
  被后台节流而失效，已排除，不能将其中的超时当作图片解码延迟。
- preview worker 仍为 2；滚动 idle gate 仍为 160 ms。测试包括暂停后的这段延迟。

## 结果

单位为 ms；每组包含三轮，数值为停止滚动到整屏 ready。

| 停止位置 | 可见图片 | 修改前三轮 | 修改后三轮 | 修改前中位数 | 修改后中位数 |
| --- | ---: | --- | --- | ---: | ---: |
| 45% | 20 | 1848 / 924 / 1067 | 453 / 397 / 371 | 1067 | 397 |
| 100% | 13 | 704 / 889 / 897 | 391 / 447 / 283 | 889 | 391 |
| 20% | 20 | 1177 / 732 / 820 | 394 / 420 / 364 | 820 | 394 |

每轮取三个停止位置中最慢的一次，修改前中位数 1067 ms，修改后 447 ms，减少约 58%。
这是该目录的三轮对照，不是跨 NAS / 平台保证，也没有修改现有性能预算或基线文件。
最终三轮的首张可见图最慢值分别为 226 / 196 / 197 ms。

## 证据和修改

1. **Sony 快速路径固定读取 2 MiB。** 抽样文件的有效 160×120 JPEG 位于 155648 字节附近，
   top-level meta box 在前 2 KiB 内，但原实现每次仍读满 2 MiB。中间诊断轮的提取阶段
   中位数约 48–63 ms，个别请求超过 480 ms；仅减少缓存维护还不能消除整屏延迟。
   现在先读取 256 KiB，在完整 meta box 和有效 JPEG 均存在时返回；其余情况继续读取到
   原有 2 MiB 上限。没有固定 JPEG 偏移，也没有降低解码回退尺寸或移除旋转信息。

2. **异步持久化仍通过锁阻塞前台。** `ArtifactPublisher::lookup_active` 读取 generation，
   `DiskMediaCache::generation` 原先与后台 publish 共用 operation mutex。
   该 mutex 跨越 publish 中的源文件验证和文件同步。去掉只读 generation 对此 mutex 的
   依赖后，一轮分段对照的首次缓存检查中位数从约 15 ms 降至约 3 ms。
   跨进程 shared cache lock 保留，clear 仍由 exclusive lock 保护。

3. **无操作 prune 也会重写所有 manifest。** 每张 artifact 完成持久化都可能触发维护，
   原实现即使未超预算也持有全局排他锁并执行全量 manifest 写入和 `sync_all`。
   现在未超容量就返回用量；超容量时也只重写实际变化的 manifest。
   单独应用这一修改没有在本案例中取得稳定的整屏改善，因此不把全部收益归因于它。

首次跳到目录末端的有效样本为 1108 ms，其中 artifact 返回后约 1–2 ms 即触发图片加载完成。
另一次单跳样本因分页/布局后的目标未进入视口而超时，没有计入对照；连续滚动测试检查实际
可见卡片，避免依赖一个固定文件名。临时 native 分段日志已移除，保留可重复运行的 grid 测试。

## 重现和验证

以下命令使用随资源预算与生命周期修复一起提交的 `grid-scroll` 探针。
仅检出先前的媒体延迟修复提交 `7553218` 时，runner 尚不支持 `--grid-scroll`。

```bash
pnpm tauri build --no-bundle
node tests/perf/perf-e2e.mjs --folder '/Volumes/photo/2026/20260607 新歌空间 7.in ShangHai Fes Vol.1 I see the light序章~RealizE初披露/05 心跳序曲Prologue' --grid-scroll --cold-cache --runs 3 --verbose
```

Runner 报告位于 `tests/perf/.reports/manual-folder.run{1,2,3}.json`。
`gridScrollFirstMs` / `gridScrollAllMs` 分别汇总每轮最慢停止位置的首图 / 整屏耗时；
各位置和文件名保留在 `grid-scroll:ready` marks 中。测试实例结束后关闭。

- 前端 159 项测试通过，TypeScript 和 release build 通过；Rust fmt / Clippy 通过。
- 新增测试覆盖 generation 不等待 publication mutex、仍等待 clear 排他锁、prune 不重写
  无变化 manifest、256 KiB 读取上限、晚 JPEG / 不完整 JPEG / 不完整元数据回退，以及真实
  Sony fixture 与原 2 MiB 扫描输出字节一致。
- Rust 完整测试的两个高清尺寸断言失败：
  `app_interim_hif_can_upgrade_to_a_satisfied_preview` 和
  `heif_without_identified_fast_representation_uses_semantic_preview_size`。
  将本次修改的两个媒体文件恢复为修改前版本进行对照，两项仍在相同断言失败；
  恢复当前实现、排除这两项后，其余 workspace 测试通过。未修改这些断言或高清预览行为。
