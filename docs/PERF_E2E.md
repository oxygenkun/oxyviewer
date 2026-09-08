# 端到端性能回归测试设计

本文档定义 OxyViewer 的端到端（E2E）性能回归测试方案，目标是在未来重构和
新增功能时，保证 `docs/PERFORMANCE.md` 中的核心交互预算不退化：

| 交互 | 预算 | 对应场景 |
| --- | --- | --- |
| 10 万文件目录首屏开始渲染 | ≤ 300 ms | `folder-open-100k` |
| 冷缓存选中图片预览 | ≤ 800 ms | `cold-preview-arw` / `cold-preview-hif` |
| 热缓存 loupe 预览 | ≤ 150 ms | `warm-loupe-jpeg` / `warm-loupe-arw` / `warm-loupe-hif-tiles` |
| loupe 全分辨率（HEIF JPEG / RAW full） | 记录 + 基线回归 | `loupe-full-hif` / `loupe-full-arw` |

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
scripts/perf-e2e.mjs (Node runner)
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
| `harness:done` | PerfHarness | 场景结束（reason: complete / timeout） |

Runner 由 mark 对计算出命名指标，`scenarios.json` 的 `budgets` 引用这些名字：

| 指标 | 计算 |
| --- | --- |
| `firstPageMs` | `harness:first-page-painted` − `folder:open-requested` |
| `firstPreviewMs` | 目标资产首个 `image:loaded` − `harness:select` |
| `previewMs` | `image:loaded`（stage=preview）− `harness:select` |
| `fullMs` | `image:loaded`（stage=full）− `harness:select`（仅记录，不计入 800 ms 预算） |
| `heifFirstTileMs` / `heifAllTilesMs` | 对应 mark − `harness:select` |
| `viewportJumpPreviewMs` | 目标网格缩略图上屏 − `harness:viewport-jump` |
| `backendDecodeMs` 等 | 取自 `preview:result` / `heif:backend-*` 的 diagnostics，仅记录 |

## 场景与图片矩阵

场景定义在 `tests/perf/scenarios.json`，起步矩阵：

| 场景 | 图片类型 | 冷/热 | 关键预算 |
| --- | --- | --- | --- |
| `folder-open-100k` | 10 万个合成 PNG（列表基准与格式无关） | 冷 | firstPageMs ≤ 300 |
| `cold-preview-arw` | `test/fixtures/media/DSC00529.ARW` | 冷 | firstPreviewMs ≤ 800 |
| `cold-preview-hif` | `tests/fixtures/DSC00449.HIF` | 冷 | firstPreviewMs ≤ 800 |
| `cold-preview-jpeg` | 合成 JPEG | 冷 | firstPreviewMs ≤ 800（走受控 original resource 路径） |
| `warm-loupe-jpeg` | 同上 | 热（连续第二次，不清应用状态） | firstPreviewMs ≤ 150 |
| `warm-loupe-arw` | 同上 | 热（连续第二次，不清缓存） | firstPreviewMs ≤ 150 |
| `warm-loupe-hif-tiles` | 同上 | 热身生成未锐化完整 JPEG；锐化开启时从 `cachedArtifact` 走 tile route | firstPreviewMs ≤ 150；tile 指标记录 |
| `loupe-full-hif` | HIF | 清空缓存后进入 loupe | 完整 JPEG 上屏记录 + 基线回归 |
| `loupe-full-arw` | ARW | 同上（awaitFull） | fullMs 仅记录（全幅显影是秒级，单列预算） |

扩展新格式（CR3/NEF/DNG/TIFF/HEIC…）：把可分发夹具放入 `test/fixtures/media`
或 `tests/fixtures`，在 `scenarios.json` 增加一条 `file` 型场景即可，无需改
代码。

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
  自身 complete；失败或缺 mark 时拒绝全部“warm”样本。HEIF tile 场景还等待明确 settle 时间，检查
  managed artifact 已出现，并拒绝 backend 不是 `cachedArtifact` 的测量样本。

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
node scripts/perf-e2e.mjs --scenario cold-preview-arw --runs 5 --verbose

# 用真实本机目录验收快速跳到未缓存区域（目标文件应位于末屏）
node scripts/perf-e2e.mjs --folder /path/to/arw-folder --select-name DSC09999.ARW \
  --scroll-end --cold-cache --runs 1 --verbose

# 重建基线（换参考机或有意的性能变化后）
node scripts/perf-e2e.mjs --update-baseline
```

本机目录模式沿用冷预览的 800 ms 交互预算：滚动到末屏时检查
`viewportJumpPreviewMs`，仅选择文件时检查 `firstPreviewMs`。

## 重构防回归工作流

1. 重构前：`node scripts/perf-e2e.mjs --update-baseline`（或确认仓库基线在
   本机有效）。
2. 重构后：`pnpm perf:e2e`。任一绝对预算或基线回归失败即视为重构引入了
   性能退化。
3. 结果符合预期时，把关键数字追加到 `docs/PERFORMANCE.md` 的
   Verification Log。

## 已知边界与后续工作

- `harness:first-page-painted` 用双 rAF 近似"开始渲染"，不替代真实
  First Contentful Paint；如需更精确可叠加 PerformanceObserver。
- HEIF 全分辨率 JPEG、RAW 全幅显影目前是"记录 + 基线回归"，待
  `docs/PERFORMANCE.md` 给出正式预算后再升级为绝对预算。
- Windows/Linux 可叠加 tauri-driver 做 UI 行为校验；性能数字仍以应用内
  探针为准。
- 10 万文件目录在 2026-09-02 的首次 release 计时约为 2.3 s，尚未达到
  300 ms 预算；优化后应在相同场景重跑并更新 `docs/PERFORMANCE.md`。
