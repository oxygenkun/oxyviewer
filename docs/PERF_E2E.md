# 端到端性能回归测试设计

本文档定义 OxyViewer 的端到端（E2E）性能回归测试方案，目标是在未来重构和
新增功能时，保证 `docs/PERFORMANCE.md` 中的核心交互预算不退化：

可用 `node tests/perf/perf-e2e.mjs --config <scenario-json> --scenario <name>` 传入外部
fixture 配置，避免把私人照片路径写入默认场景。Windows runner 等待测试子进程退出后再清理
隔离状态，对短暂残留的文件句柄做有界重试；不会清理普通用户的 data/cache。
JPEG 合成仍依赖 macOS `sips`，Windows 验证应使用 `file` fixture。真实滚轮采样另见
[JPEG/RAW 小图验收](research/jpeg-thumbnail-scroll-2026-09-12.md)。

| 交互 | 预算 | 对应场景 |
| --- | --- | --- |
| 10 万文件目录首屏开始渲染 | ≤ 300 ms | `folder-open-100k` |
| 冷缓存选中图片预览 | ≤ 800 ms | `cold-preview-arw` / `cold-preview-hif` |
| 热缓存 loupe 预览 | ≤ 150 ms | `warm-loupe-jpeg` / `warm-loupe-arw` / `warm-loupe-hif-artifact` |
| loupe 全分辨率（HEIF JPEG / RAW full） | 记录 + 基线回归 | `loupe-full-hif` / `loupe-full-arw` |

## 目录布局

runner、配置、夹具和报告都属于同一个单元，全部放在 `tests/perf/`：

| 路径 | 用途 |
| --- | --- |
| `perf-e2e.mjs` | E2E runner；`pnpm perf:e2e` 的入口，从 `import.meta.url` 推导仓库根，可在任意目录运行 |
| `perf-scroll.mjs` | 独立 CDP 滚轮探针，附着到显式给出的 WebView 调试端口；用于真实滚轮采样 |
| `raw-backend-bench.ps1` | Windows RAW 后端对比基准（WIC / LibRaw）；从仓库根运行，默认写入 `tests/perf/.reports/raw-backend` |
| `scenarios.json` | 场景定义与绝对预算（入库） |
| `baseline.json` | 基线 median（入库，`--update-baseline` 重建） |
| `generated/` | runner 生成的合成夹具（已 gitignore） |
| `.reports/` | 每次运行的 JSON 报告、stderr 日志和隔离 data/cache（已 gitignore） |

Node 入口必须是 `.mjs`：根 `package.json` 没有 `"type": "module"`。

## 为什么不用 tauri-driver / Playwright

- tauri-driver 目前只支持 Linux（WebKitGTK）和 Windows（WebView2）的
  WebDriver 协议；**macOS 的 WKWebView 没有 WebDriver 支持**，而参考机是
  Mac，冷解码预算也主要在 macOS 原生解码路径（ImageIO/Core Image）上。
- Playwright 驱动 vite 浏览器模式必须 mock IPC，测不到 Rust 解码、缓存、
  优先级队列，冷预览指标失真。

因此采用**应用内自驱动 harness**：用环境变量把场景注入真实打包应用，应用
内的 `PerfHarness` 组件像用户一样驱动真实的 store / React Query / IPC /
渲染管线，探针记录时间戳，最后由 Rust 命令把报告写到磁盘。这是真实全链路
E2E（真实后端、真实 IPC、真实 WebView 渲染），且跨平台一致。Windows/Linux
的 CI 后续可以在此之上叠加 tauri-driver 做 UI 级校验，但性能数字以本方案
为准。

## 架构

```
tests/perf/perf-e2e.mjs (Node runner)
  │  1. 准备夹具目录（合成 10 万文件 / 硬链接 RAW、HIF 夹具）
  │  2. coldCache 场景：删除 app_cache_dir/previews
  │  3. 以 OXY_PERF_SCENARIO=<json> 启动 target/release/oxyviewer
  ▼
main.tsx ── get_perf_scenario (Rust, 读 env) ──► 有场景则渲染 <App perfScenario>
  ▼
PerfHarness 组件（真实 UI 路径）
  │  openPath(folder) → 等首屏数据 → 双 rAF 标记首屏 paint
  │  → select(asset) → setView("loupe") → 等 awaitMarks
  ▼
perfProbe（src/lib/perfProbe.ts，未激活时零开销）
  │  埋点：api.ts(openFolder/listAssets/generatedPreview/startHeifDecode)
  │       Thumbnail(image:loaded)                         PerfHarness
  ▼
write_perf_report (Rust, 写 JSON)
  ▼
runner 汇总 N 次运行 → median/p95 → 绝对预算 + 基线回归判定 → 退出码
```

## 探针与指标字典

所有探针使用 `performance.now()`，报告中的 `t` 相对页面加载起点。

| Mark | 埋点位置 | 含义 |
| --- | --- | --- |
| `harness:start` | PerfHarness | 场景开始（紧接 openFolder 之前） |
| `folder:open-requested` / `folder:open-returned` | api.openFolder | 打开文件夹 IPC 往返 |
| `assets:first-page-returned` | api.listAssets | 第一页（250 条）摘要返回 |
| `harness:first-page-painted` | PerfHarness | 首屏数据提交后双 rAF，近似"开始渲染" |
| `harness:select` | PerfHarness | 选中目标图片（冷/热预览计时起点） |
| `harness:viewport-jump` | PerfHarness | 网格快速跳到目录末端（冷区域计时起点） |
| `preview:queued` / `preview:result` | api.generatedPreview | 预览请求入队 / 后端返回（含 `diagnostics`） |
| `image:loaded` | Thumbnail.onLoad | 某一语义等级上屏（detail 含 `stage` / `renderLevel`） |
| `image:loaded@full` | Thumbnail | HEIF/RAW 完整 JPEG 上屏 |
| `image:source-committed` | Thumbnail | 新资源 URL 已提交给待显示的 img |
| `image:load-event` | Thumbnail | img load；包含 Resource Timing 和原生 dispatch/materialize Server-Timing |
| `image:decode-complete` | Thumbnail | img.decode 完成；decodeAfterLoadMs 为 load 之后的解码等待 |
| `harness:done` | PerfHarness | 场景结束（reason: complete / timeout） |

Runner 由 mark 对计算出命名指标，`scenarios.json` 的 `budgets` 引用这些名字：

`Server-Timing` 与 `Timing-Allow-Origin` 仅在显式性能场景中启用；常规请求不增加计时采样。
性能探针扩大 Resource Timing 缓冲到 8192 项，避免整目录预热后默认 250 项缓冲已满。
`image:loaded` 是现有解码就绪/状态更新标记，不能等同于 GPU present；最终画面另做截图检查。

| 指标 | 计算 |
| --- | --- |
| `firstPageMs` | `harness:first-page-painted` − `folder:open-requested` |
| `firstPreviewMs` | 目标资产首个 `image:loaded` − `harness:select` |
| `previewMs` | `image:loaded`（stage=preview）− `harness:select` |
| `fullMs` | `image:loaded`（stage=full）− `harness:select`（仅记录，不计入 800 ms 预算） |
| `heifFirstTileMs` / `heifAllTilesMs` | 对应 mark − `harness:select` |
| `viewportJumpPreviewMs` | 目标网格缩略图上屏 − `harness:viewport-jump` |
| `gridScrollFirstMs` / `gridScrollAllMs` | 连续滚动停止后首张 / 全部可见图片 ready；取每轮最慢停止位置 |
| `backendDecodeMs` 等 | 取自 `preview:result` / `heif:backend-*` 的 diagnostics，仅记录 |

## 场景与图片矩阵

任意真实目录可使用 `node tests/perf/perf-e2e.mjs --folder '<path>' --grid-scroll --cold-cache --runs 3`。
`grid-scroll` 每段连续滚动 32 次、间隔 16 ms，依次停在 45%、100%、20%，再检查整个实际可见
视口的 displayed images。每 25 ms 采样，15 秒内未补齐则失败；只记录耗时，不默认给任意 NAS
套用本地 SSD 预算。`--grid-scroll` 不与 `--scroll-end` / `--select-name` 混用。
这补充了原资源预算压力测试每次滚动等待 350 ms、主要检查内存上限的场景。
案例与前后对照见 [NAS HIF grid 延迟](research/nas-hif-grid-latency-2026-09-09.md)。

场景定义在 `tests/perf/scenarios.json`，起步矩阵：

| 场景 | 图片类型 | 冷/热 | 关键预算 |
| --- | --- | --- | --- |
| `folder-open-100k` | 10 万个合成 PNG（列表基准与格式无关） | 冷 | firstPageMs ≤ 300 |
| `cold-preview-arw` | `tests/fixtures/DSC00529.ARW` | 冷 | firstPreviewMs ≤ 800 |
| `cold-preview-hif` | `tests/fixtures/DSC00449.HIF` | 冷 | firstPreviewMs ≤ 800 |
| `cold-preview-jpeg` | 合成 JPEG | 冷 | firstPreviewMs ≤ 800（走受控 original resource 路径） |
| `warm-loupe-jpeg` | 同上 | 热（连续第二次，不清应用状态） | firstPreviewMs ≤ 150 |
| `warm-loupe-arw` | 同上 | 热（连续第二次，不清缓存） | firstPreviewMs ≤ 150 |
| `warm-loupe-hif-artifact` | 同上 | 热身生成 Display full JPEG；重开后直接显示完整 artifact | firstPreviewMs ≤ 150；必须命中 full cache 并加载 full 图 |
| `loupe-full-hif` | HIF | 清空缓存后进入 loupe | 完整 JPEG 上屏记录 + 基线回归 |
| `loupe-full-arw` | ARW | 同上（awaitFull） | fullMs 仅记录（全幅显影是秒级，单列预算） |

扩展新格式（CR3/NEF/DNG/TIFF/HEIC…）：把可分发夹具放入 `tests/fixtures`，
在 `scenarios.json` 增加一条 `file` 型场景即可，无需改代码。

## 夹具与缓存管理

- **合成大目录**：runner 生成 `tests/perf/generated/folder-open-100k-100000/`
  （已 gitignore），首个文件为真实 PNG 字节，其余为硬链接，生成一次后复用。
  打开是非递归分页摘要，文件内容不影响列表性能；缩略图失败回退也是 UI
  的一部分。如需真实图片矩阵，把生成器指向样本库即可。
- **单文件场景**：runner 在 `generated/` 下为每个场景建目录并硬链接夹具
  文件，避免每次复制几十 MB。
- **隔离状态**：runner 为每个场景设置 `OXY_PERF_DATA_DIR` / `OXY_PERF_CACHE_DIR`，应用只在同时
  存在 `OXY_PERF_SCENARIO` 时接受覆盖。目录位于 `tests/perf/.reports/.runtime/<scenario>/`，不会打开
  或清除正常用户的 SQLite、设置或 preview cache。
- **冷缓存**：`coldCache: true` 每次运行前删除该场景隔离的 data/cache，不能触及平台默认目录。
- **热缓存**：隔离目录中的 warmup 进程退出后由测量进程重开，验证 SQLite DTO 恢复、当前进程
  resource 重新登记和磁盘 artifact 热命中，而不只是同进程内存命中。runner 必须先验证 warmup
  自身 complete；失败或缺 mark 时拒绝全部“warm”样本。HEIF full 场景还等待明确 settle 时间，检查
  managed artifact 已出现，并要求测量样本同时出现 `heif:full-cache-hit` 与 `image:loaded@full`。

## 判定规则

1. **绝对预算**：`budgets` 中的指标取 N 次运行的 median，超过即失败。预算
   值与 `docs/PERFORMANCE.md` 一一对应，修改预算必须先改文档。
2. **基线回归**：所有记录指标与 `tests/perf/baseline.json` 中的 median 对比，
   退化超过 `regressionFactor`（默认 1.2，即 20%）即失败。首次运行或更换
   参考机后用 `--update-baseline` 重建基线并提交。
3. 任何场景出现 `harness:done reason=timeout`、进程崩溃或缺失必等 mark，
   该次运行记为失败。
4. 建议本机参考机跑 release 包做门禁；CI 共享 Runner 性能抖动大，建议先以
   record-only（只上传报告不判失败）接入，稳定后再开门禁。

## 运行方式

```bash
# 首次：用 Tauri 正式流程构建嵌入前端资源的 release 应用。
# 不要用裸 cargo build；它会保留 devUrl 并打开 localhost:15142。
pnpm tauri build --no-bundle

# 跑全部场景（每个场景默认 3 次）
pnpm perf:e2e

# 单场景、自定义次数、查看每次明细
node tests/perf/perf-e2e.mjs --scenario cold-preview-arw --runs 5 --verbose

# 用真实本机目录验收快速跳到未缓存区域（目标文件应位于末屏）
node tests/perf/perf-e2e.mjs --folder /path/to/arw-folder --select-name DSC09999.ARW \
  --scroll-end --cold-cache --runs 1 --verbose

# 重建基线（换参考机或有意的性能变化后）
node tests/perf/perf-e2e.mjs --update-baseline
```

本机目录模式沿用冷预览的 800 ms 交互预算：滚动到末屏时检查
`viewportJumpPreviewMs`，仅选择文件时检查 `firstPreviewMs`。

## 重构防回归工作流

1. 重构前：`node tests/perf/perf-e2e.mjs --update-baseline`（或确认仓库基线在
   本机有效）。
2. 重构后：`pnpm perf:e2e`。任一绝对预算或基线回归失败即视为重构引入了
   性能退化。
3. 结果符合预期时，把关键数字追加到 `docs/PERFORMANCE.md` 的
   historical verification log。

## 已知边界与后续工作

- `harness:first-page-painted` 用双 rAF 近似"开始渲染"，不替代真实
  First Contentful Paint；如需更精确可叠加 PerformanceObserver。
- HEIF 全分辨率 JPEG、RAW 全幅显影目前是"记录 + 基线回归"，待
  `docs/PERFORMANCE.md` 给出正式预算后再升级为绝对预算。
- Windows/Linux 可叠加 tauri-driver 做 UI 行为校验；性能数字仍以应用内
  探针为准。
- 10 万文件目录在 2026-09-02 的首次 release 计时约为 2.3 s，尚未达到
  300 ms 预算；优化后应在相同场景重跑并更新 `docs/PERFORMANCE.md`。


## Resource registry 压力验收

```bash
pnpm tauri build --no-bundle
node tests/perf/perf-e2e.mjs --scenario resource-stress-grid --scenario resource-stress-hif --runs 1
```

grid 生成 600 个独立路径的 JPEG，使用生产的高密度竖图网格和固定 overscan，逐视口滚动，
要求至少 500 个不同文件实际触发 image onLoad。HIF 使用真实 fixture 的 80 个独立路径，先每 120 ms
切换选择，再要求末尾 8 张各自 Full 实际显示。测试不更改照片内容；生成的路径位于 ignored perf fixture
目录，hardlink 只用于复用输入字节。

脚本使用隔离 data/cache，并在最多 4 个真实 resource URL 读取期间执行 clear 和 prune。它检查 registry
peak entries/encoded bytes 不越过配置，等待发布宽限和一次 10 秒心跳后检查数量回落到 displayed/pending
工作集附近。`resource:stats` / `resource:stress-complete` 包含真实计数与上限；错误写入
`resource:stress-failed` 并使场景失败。每轮 stderr 保存在报告旁的 `.json.stderr.log`，容量耗尽也使压力场景
失败。`get_media_resource_stats` 仅在 debug 或显式 `OXY_PERF_SCENARIO` 下可用，不启动后台采样器。

这些是 macOS packaged WebView 压力场景；Windows/Linux 原生环境须分别运行，不能用本机报告替代。

### UI cache return and small-thumbnail pressure

`navigation-cache-jpeg` and `navigation-cache-hif` warm two assets through the
real loupe, then alternate four times (HEIF runs both unsharpened artifact and sharpened
canvas presentations). Each return must commit the correct asset
and a displayed image within 150 ms; JPEG URLs must match their warmed source,
and HEIF must emit `heif:memory-cache-hit` instead of starting a new tile session.
Marks `navigation:cache-return` retain the individual elapsed times.
`resource-stress-small-grid` scrolls 600 small JPEGs to exercise native registry
slots independently of the decoded-pixel budget, then runs the existing cache
maintenance/read and resource-settlement checks.

`filmstrip-scroll-hif` exercises the same continuous scroll probe horizontally
with 80 HIF sources. Both scroll probes emit `*:moving` samples (displayed versus
visible images) as well as stopped-viewport readiness. A locked or backgrounded
WebView can clamp timers; such runs are not valid fast-scroll measurements.

## Loupe 大图缩放回归

外部场景配置可使用 `resourceStress: "loupe-zoom"` 和
`awaitMarks: ["resource:stress-complete"]`，fixture 指向单张真实大尺寸 RAW。
探针选择该图并等待 fullReady，然后通过实际 wheel 事件依次缩放至
50%、73%、91%、100%、150%、200%、400%，检查图片宽高与原始像素尺寸一致，
并记录 `loupe:zoom-sample`（实际宽高、render 宽高、源尺寸、objectFit）。
使用 `node tests/perf/perf-e2e.mjs --config <external-json> --scenario <name>` 运行。

该探针只证明 DOM 几何正确，不能证明最终绘制的像素比例。macOS WKWebView
曾在 DOM 完全正确时将 `object-fit: contain` 的大图拉伸；必须另外在 Release
窗口检查 73% 前后及 100% / 200% 的实际画面，并与同图低倍率的细节比例对照。
