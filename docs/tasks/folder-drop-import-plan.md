# 拖拽文件夹导入方案

状态：方案定稿（2026-09-16）；R1 已核对，Phase 1 与 Phase 2 按本文实施。

本方案描述 OxyViewer 桌面端把 Finder / 资源管理器中的文件夹拖入窗口即完成“导入”的能力：
把文件夹注册为图库根、在后台建立索引、让它在侧边栏立即可见，并在整个过程中给出**可见、
真实、不阻塞浏览**的导入反馈。性能与数据边界仍以
[浏览与媒体性能约束](../PERFORMANCE_INVARIANTS.md)和
[状态、数据与安全边界](../architecture/05-data-and-state.md)为准；本文不得覆盖这些契约。

## 1. 目标与范围

### 1.1 目标

- 从系统文件管理器拖入一个或多个文件夹，松手即导入；
- 导入 = 注册图库根 + 后台索引 + 侧边栏出现 + 激活第一个拖入的文件夹；
- 拖动过程中有明确的“可放下”遮罩，松手后每个文件夹有独立的阶段进度与结果；
- 单个文件夹失败（卷离线、权限拒绝、路径消失）不影响其余文件夹；
- 整个过程不阻塞浏览：索引继续走后台队列并给前台让路。

### 1.2 非目标

- **不复制、不移动原始文件**。本方案的“导入”是就地引用加索引。复制式导入（受管图库目录、
  冲突策略、原文件保留策略）是独立特性，见 §8。
- 不改变侧边栏“拖拽排序文件夹”的既有交互（自研 pointer events，与本方案不冲突）。
- 不处理系统级“用 OxyViewer 打开”注册、拖到 Dock/任务栏图标、跨窗口拖放。

## 2. 现状与约束（已核对）

| 事项 | 位置 | 结论 |
| --- | --- | --- |
| 现有拖放处理 | 全仓库 | 不存在任何 drag/drop 处理（`onDrop` / `dataTransfer` / `onDragDropEvent` 均无匹配） |
| 打开文件夹 | `apps/desktop/src/App.tsx` 的 `openPath()` → `commands/folder.rs` 的 `open_folder` | 便宜、非递归，只建 session；不得在其上追加扫描 |
| 图库注册 | `commands/library.rs` 的 `add_library_root` | `INSERT OR IGNORE`（幂等），并在此处 `schedule` 索引 |
| 索引队列 | `apps/desktop/src-tauri/src/jobs.rs` 的 `LibraryIndexQueue` | 每根串行、`foreground.wait_for_background()` 主动让位前台 |
| 索引进度数据 | `crates/oxy-library/src/lib.rs` 的 `IndexProgress` / `index_root_inner` | 内部每目录回调一次，仅用于队列诊断快照；**导入不解析它**（见 §3.2） |
| 索引进度事件 | 同上 `jobs.rs` | 只有“进入 Assets 阶段”和“整根完成”两个里程碑事件，事件负载只含根路径与计数 |
| 状态栏提示通道 | `App.tsx` 的 `notice` + `styles/status.css` | 错误/状态提示统一落在这里，可展开看明细；导入复用它 |
| 恢复态 UI 可复用 | `apps/desktop/src/lib/browse/folderRestoration.ts`、`components/browsing/Sidebar.tsx` | `restoring / ready / failed` 逐根独立发布 |
| 侧栏文件夹排序 | `components/browsing/Sidebar.tsx` | 自研 pointer events，不是 HTML5 DnD，与原生拖放不冲突 |
| 索引可取消性 | `crates/oxy-library/src/lib.rs` | 索引只在 `contains_root` 检查点可被“移除根”打断；`cancel_job` 只覆盖通用作业注册表 |
| 浏览器 demo 模式 | `apps/desktop/src/lib/api.ts` 的 `isTauri()` 分支 | 新能力必须提供 demo 分支，否则 `pnpm dev` 浏览器调试不可用 |
| i18n | `apps/desktop/src/lib/i18n.ts` | `translate(locale, key)` 无参数；占位符由调用点 `.replace("{x}", …)` 完成 |

## 3. 交互设计

### 3.1 拖入提示（`enter` / `over` / `leave`）

```text
┌────────────────────────────── 整窗 ──────────────────────────────┐
│  （虚线框沿应用边缘内缩 9px；面板 = --panel-raised 80%，无模糊）    │
│                                                                  │
│                            ⬇  ⃞                                 │
│                      松开以导入文件夹                             │
│                 将加入图库并建立索引，原始文件不会被移动             │
│                    ▸ 2024-北海道   ▸ Weddings                    │
│                                                                  │
└──────────────────────────────────────────────────────────────────┘
```

- **卡片本身就覆盖整窗口**：`.drop-overlay__card` 以 `inset: 9px` 铺满窗口（留 9px 只为不让
  虚线被窗口边缘裁切），`border-radius: 15px` + 1.5px 虚线（`--accent` 56%）让虚框贴着整个应用
  边缘；面板用 `--panel-raised` 80% 的**纯半透明填充**（不用 `backdrop-filter`，省掉全窗模糊的
  GPU 合成），应用内容仍能透出来。图标、标题、说明与文件夹 chip 在该卡片内垂直居中。
  透出程度由这一个百分比控制：觉得背景文字干扰就往上调，觉得太实就往下调。
- 覆盖层 `z-index: 10000`，高于 debug 队列按钮（9999）等常驻浮层，确保拖动时真正铺满；
  同时必须 `pointer-events: none`，不得吞掉落点。
- `enter` 携带 `paths`，立即显示卡片并用 basename 渲染 chip；**此处不做 `stat`**，避免拖到
  离线网络卷上时卡住 UI。是否为目录留到 `drop` 时校验。
- `over` 以指针频率重复触发，此处**直接忽略**：卡片铺满整窗，没有任何需要跟随指针的视觉，
  因此既不改状态也不改样式。React 状态只在 `enter` / `drop` / `leave` 切换。（早期实现有一圈
  跟随指针的光晕，已按要求移除。）
- `leave`：约 120ms 淡出；导入进行中的底栏提示不受影响。

### 3.2 导入过程与结果：全部在底栏

导入**没有浮层面板，也不解析索引进度**。整个过程复用应用已有的状态栏提示通道
（`.statusbar__notice`，与目录快照、错误提示同一套 UI）：收起时是一行状态，点开是每个文件夹一行。

```text
底栏（进行中）：  正在导入 3 个文件夹
底栏（完成） ：  已导入 3 个文件夹 · 12,480 张照片

点开提示后的明细：
  2024-北海道 — 正在加入图库…
  Weddings   — 正在建立索引…
  Vacation   — 完成 · 12,480 张照片
  NAS-Archive — 无法访问：No such file or directory
```

- **进行中**：`kind: "status"`，只有“正在导入 N 个文件夹”这一行，**没有百分比、没有阶段计数**；
  索引完成后才由既有的 `library-index-updated` 事件补上照片数。
- **成功**：`已导入 N 个文件夹 · M 张照片`，6 秒后自动退场。
- **部分失败**：`已导入 N 个文件夹 · K 项失败`，`kind: "error"`；展开后是 `名称 — 原因` 明细加
  “重试”按钮（一次性重试所有失败行）与关闭按钮，失败不静默丢弃。
- **整次失败**（文件夹打不开）：直接显示错误文本，`kind: "error"`，可关闭。
- 关闭提示会一并清除导入状态；提示面板限高 42vh 并可滚动，导入几百个文件夹也不会撑爆底栏。
- 每个文件夹完成即逐根发布，对应文件夹立刻出现在侧边栏；全部结束后激活**第一个**文件夹，
  并关闭首次使用引导。
- 不提供“从侧边栏移除”的软撤销：侧边栏每行已有移除入口，不重复放一个只在数秒内可用的操作。
- **两个入口完全一致**：侧边栏 `+`、空态与错误态的“打开文件夹”按钮（`handleOpen`）与拖拽
  （`useFolderDrop`）都只调用 `startFolderImport(paths)`，按钮传一个路径、拖拽传 payload 顺序的
  路径数组，**没有按钮专属或拖拽专属的分支**。拖入卡片是拖拽独有的视觉提示，不属于后续逻辑。
  只有性能 harness 保留不带导入反馈的原始 `openPath`，避免它的场景目录被注册或产生提示噪音。

### 3.3 边界态

| 情况 | 行为 |
| --- | --- |
| 再次导入已在图库的文件夹 | `add_root` 幂等，且 `root_needs_index` 已完成时不重排队；等同再打开一次并激活它 |
| 一次拖入多个文件夹 | 按 payload 顺序逐个打开与注册（顺序 = 侧边栏顺序）；**不判断它们之间的嵌套或父子关系** |
| 混入普通文件 | `open_folder` 对该路径返回错误，该行标 `failed` 并给出原因，其余行不受影响 |
| 卷离线 / 权限拒绝 / 路径消失 | 同上，逐行失败，可重试 |
| 超大目录 / 网络卷 | 索引在后台队列串行执行并主动让位前台；导入本身只做打开与注册 |
| 取消 | 不提供中途取消索引；只能“完成后从侧边栏移除”（移除根会在检查点终止索引） |
| 引导页 / 设置面板打开时拖入 | 拖入卡片 `z-index: 10000`，层级高于它们，落点始终生效 |

### 3.4 文案与无障碍

- 新增 i18n 键中英双份；占位符沿用 `.replace("{x}", …)`。
- 状态栏提示沿用既有 `role="status"` 语义；拖入卡片为 `aria-hidden`（纯视觉提示，实际状态由底栏播报）。
- 遵守 `prefers-reduced-motion`：关闭呼吸/位移动画，保留状态文字。

## 4. 契约与实现

### 4.1 领域契约（`oxy-domain`，camelCase）

导入**没有新增任何 Rust 契约**。文件夹导入复用既有链路：

```rust
FolderSession        // open_folder 的返回，既有
LibraryIndexUpdate   // { root_path, asset_count, directory_count }，既有完成事件负载
```

早期版本曾新增 `DroppedPathsResolution` / `DroppedPathRejection`（解析拖入路径、折叠嵌套）与
`LibraryIndexProgress` / `LibraryIndexStage`（节流实时进度）。两者都已按“不做额外操作、不解析进度”
删除，前端也不再调用任何解析命令。

### 4.2 Rust

导入路径上 `src-tauri` 与各 crate **一行都没改**：

- `open_folder`（`commands/folder.rs`）：canonicalize + 建 session，便宜、非递归；
- `add_library_root`（`commands/library.rs`）：`INSERT OR IGNORE` 幂等注册，并 `schedule` 索引；
- `LibraryIndexQueue`（`jobs.rs`）：每根串行、主动让位前台，仍只发既有两个里程碑事件。

### 4.3 前端

- `lib/api.ts`：新增 `onWindowDragDrop`（包装 `getCurrentWebview().onDragDropEvent`，带 `isTauri()`
  demo 分支）；`chooseFolder` 与文件夹打开相关包装保持不变。
- `lib/folderImport.ts`（新）：纯函数状态机
  `registering → indexing → ready | failed`，只做“打开一个文件夹”这件事的记账；不含解析、去重、
  嵌套判断或进度换算。`attachImportRoot` 按 payload 顺序把后端返回的 canonical root 绑到对应行。
- `lib/folderImportNotice.ts`（新）：把导入状态压成底栏那一行 `{ kind, message, detail, retry }`。
- `lib/useFolderDrop.ts`（新）：注册/注销原生拖放监听，产出拖入卡片所需的视图状态；非 Tauri 环境
  退回 HTML5 事件（demo 用 `DataTransferItem` 推断文件夹名）。
- `components/ImportOverlay.tsx`（新）：整窗拖入卡片。
- `App.tsx`：`startFolderImport(paths)` 是**唯一**导入实现，`+` 按钮（`handleOpen`）与拖拽
  （`useFolderDrop`）都只调用它；`openPath` 保留但仅服务性能 harness。

### 4.4 时序

```text
OS drag enter ──▶ 拖入卡片（paths 只用于显示文件夹名）
OS drop        ──┐
+ 按钮 / 选择器 ──┴─▶ startFolderImport(paths)
                     ──▶ 逐个：open_folder（建 session）→ add_library_root（注册 + 排队索引）
                     ──▶ 底栏“正在导入 N 个文件夹”
                     ──▶ library-index-updated（整根完成）──▶ 底栏结果 + 侧边栏已就位
```

## 5. 分阶段实施

**Phase 1（已实现）**
原生拖放监听 + 整窗拖入卡片 + `startFolderImport(paths)`（逐个 `open_folder` → `add_library_root`）
+ 侧边栏出现 + 底栏“正在导入 / 已导入”提示。
验收：拖入 3 个文件夹 → 卡片出现、松手后逐个进侧边栏、底栏给出结果，期间浏览不卡。

**Phase 2（已按需求收窄）**
原计划的“实时进度事件 + 百分比 + 逐根阶段计数”**未采用并已回退**：导入不再解析索引进度，
只在整根完成时由既有事件补上照片数，见 §3.2。

**Phase 3（已按需求收窄为“不做”）**
不做嵌套折叠文案、不做“已在图库中”判定、不做图片拖入转父目录；剩余可选项只有真机多平台手工矩阵。

## 6. 性能与不变量对照

| 不变量 | 本方案如何满足 |
| --- | --- |
| 打开文件夹必须便宜、非递归、分页 | 导入对每个路径只调用既有的 `open_folder`（canonicalize + 建 session），没有新增扫描 |
| 不在交互路径上索引 | 注册后仍走 `LibraryIndexQueue` 后台队列；索引前 `foreground.wait_for_background()` |
| IPC 不传图像字节 | 拖放事件只传路径 |
| 文件发现与安全操作走 `oxy-fs` | 前端不判断路径类型；失败由 `open_folder` 返回的错误决定 |
| 阻塞/CPU 工作离开 async 与 UI 线程 | 前端只做 `await` 既有命令；`over` 事件不触发 React 渲染 |
| 优先级/取消/世代语义不变 | 导入不触碰索引内部的取消与世代逻辑，也没有新增播报通道 |
| 分层与契约 | 未新增 Rust 契约或命令；IPC 包装集中在 `api.ts`，状态机是纯函数并就近测试 |

## 7. 验证矩阵

- 单测：`lib/folderImport.test.ts`（开始、绑定 canonical root、完成计数、失败与重试）、
  `lib/folderImportNotice.test.ts`（进行中/结算/部分失败/整次失败）、`lib/useFolderDrop.test.tsx`
  （`enter`/`over`/`drop`/`leave`，含“`over` 不重渲染”）、`ImportOverlay.test.tsx`。
- 命令：`pnpm -C apps/desktop test`、`pnpm -C apps/desktop check`、`cargo clippy -p oxyviewer
  --all-targets`、`cargo test -p oxyviewer --lib`、`cargo test -p oxy-fs`。
- 手工：macOS / Windows × Finder / Explorer × 单根、多根、含文件、超大目录、网络卷离线；
  确认期间前台浏览与预览队列无退化，并确认 `+` 按钮与拖拽的后续行为一致。
- 性能：按 [CONTRIBUTING.md](../../CONTRIBUTING.md) 用 release 构建 + 真实大目录 fixture 对照
  [PERFORMANCE.md](../PERFORMANCE.md)；导入路径只调用既有命令，不新增需单独测量的工作。

## 8. 风险与待定项

- **R1 原生拖放事件可用性 —— 已核对（静态证据，2026-09-16）**
  - `tauri.conf.json` 未设置 `dragDropEnabled`；`tauri-utils` 的 `WindowConfig` 默认
    `drag_drop_enabled: true`，原生拖放处理器因此处于启用状态，OS 落点会被转换成
    `tauri://drag-enter|over|drop|leave`。
  - 事件由 `tauri` 的 `manager/window.rs` 与 `manager/webview.rs` 通过
    `emit_to_window` / `emit_to_webview` 发出；前端 `Webview.listen` 走 `plugin:event|listen`，
    而 `capabilities/default.json` 的 `core:default` 已包含 `core:event:default`
    （`allow-listen` / `allow-unlisten` / `allow-emit` / `allow-emit-to`），
    **无需新增 capability**。该命令的 `target` 参数只用于路由（`Any` / `Window` / `Webview`），
    不参与权限判定，因此 webview 维度监听与仓库已有的全局监听受同一权限保护。
  - `wry` 的 `wkwebview` / `webview2` / `webkitgtk` 三个后端均以
    `DragDropEvent::Enter { paths, position }` 形式提供路径，因此“`enter` 只用于显示文件夹名、
    `drop` 才把路径交给导入管线”的设计在三平台都成立。
  - 仍需真机确认的仅是行为细节（多显示器坐标、Windows 上高频 `over` 的事件量），
    它们不改变契约；若不满足，退回在 Rust 侧 `WindowEvent::DragDrop` 转发自定义事件，
    前端结构不变。
- **R2 `over` 事件**：卡片铺满整窗后没有任何需要跟随指针的视觉，`over` 直接忽略；实现时不得在 `over` 上 setState。
- **R3 索引不可取消**：v1 明确不承诺中途取消，移除根会在检查点终止索引。
- **R4 “导入”语义**：本方案按“就地引用 + 索引”实现。若产品要的是**复制进受管图库目录**
  （名冲突策略、原文件保留、校验和、进度中断续传），需另立设计，本方案可作为它的 UI 外壳。
- **R5 demo 模式**：浏览器 demo 无法取得真实路径，只能用推断出的展示路径驱动视觉验收，
  不得据此判断真实导入行为；demo 里没有后台索引，因此导入会立即结算。

## 9. 文档与登记

- 本文登记在 [任务计划索引](README.md)。
- 落地后若引入新的 IPC 事件与领域契约，需同步
  [架构总览](../ARCHITECTURE.md) 中相关的 IPC / 事件清单；行为契约变化同步
  [性能约束](../PERFORMANCE_INVARIANTS.md)。
- 完成实施后按仓库惯例把本文移入 `docs/archive/plans/`，或在 §10 记录完成状态。

## 10. 实施记录

- 2026-09-16：方案定稿；R1 核对完成（见 §8）。
- 2026-09-16：Phase 1 与 Phase 2 落地。
  - Rust：`oxy-domain` 新增 `DroppedPathsResolution` / `DroppedPathRejection` /
    `LibraryIndexProgress` / `LibraryIndexStage`；`oxy-fs::resolve_dropped_roots` 做
    canonicalize、目录判定、嵌套折叠与保序去重；`resolve_drop_paths` 命令在 `spawn_blocking`
    中调用它；`LibraryIndexQueue` 新增 ≥150ms 节流的 `library-index-progress`（阶段切换、根切换
    立即发送，锁外 emit）。
  - 前端：`lib/folderImport.ts` 纯函数状态机、`lib/useFolderDrop.ts` 原生拖放订阅（浏览器 demo
    退回 HTML5 事件）、`components/ImportOverlay.tsx` 与 `components/ImportPanel.tsx`、
    `styles/import.css`、中英文案；`App.tsx` 把 `openPath` 拆成会抛出错误的 `registerRoot`
    与保留原行为的对话框入口，导入按拖入顺序逐根注册并激活第一个根。
  - 验证：`cargo clippy -p oxy-fs -p oxyviewer --all-targets` 通过；
    `cargo test -p oxy-fs`（33 通过，含 3 个 `drops_*`）、
    `cargo test -p oxyviewer --lib`（45 通过，含进度节流用例）通过；
    `pnpm -C apps/desktop check` 与 `pnpm -C apps/desktop build` 通过；
    `pnpm -C apps/desktop test` 330 通过（新增状态机 13、面板/遮罩 6、原生拖放 hook 3，
    其中 hook 用例覆盖“`over` 不触发重渲染”与卸载退订）。
  - 未完成：真机手工矩阵（macOS / Windows 实际拖放、多显示器坐标、超大目录、网络卷离线）与
    release 构建下的性能对照；Phase 3 打磨项仍未开始。因此本文继续留在 `docs/tasks/`。
- 2026-09-16：交互几何与主题对齐修正（浏览器 demo 实测截图）。
  - 虚框改为 §3.1 约定的**整窗内缩投放框**：`.drop-overlay::before` 以 `inset: 13px` 沿应用边缘
    画一圈 1.5px 虚线并带 13px 圆角；居中提示卡片改为 `--panel-raised` 实底面板、不再自带虚线，
    避免“小卡片 + 整窗遮罩”的两种框互相竞争。
  - 变量与排版改用仓库真实主题：`--bg` / `--panel-raised` / `--surface-muted` / `--surface-hover` /
    `--line` / `--line-control` / `--text` / `--muted` / `--faint` / `--accent` / `--popover-shadow`，
    字号收敛到应用的 0.5625–0.75rem 档（此前误用了不存在的 `--surface` / `--surface-raised` /
    `--border` / `--danger`，只能吃回退色）。
  - 实测：`pnpm dev` 下合成 dragenter/drop 截图确认投放框、光晕、卡片与右下导入面板均按设计渲染，
    demo 流程可把文件夹加入侧边栏并把面板收束为“已导入 N 个文件夹 · M 张照片”；
    `pnpm -C apps/desktop check` / `test`（330） / `build` 重新通过。
- 2026-09-16：投放提示改为紧凑卡片（第二次几何修正，**已被下一次修正取代**）。
  - 反馈：整窗内缩虚线框 + 整窗半透明遮罩在真实窗口里呈现为“贴着整个 app 边缘的虚框 + 一块接近
    窗口大小的半透明卡片”，遮挡应用内容。
  - 结果：删除 `.drop-overlay` 的整窗背景与 `::before` 边缘虚线框；只保留 `340px` 宽的居中虚线
    卡片（`--panel-raised` 76% + `backdrop-filter`），光晕缩到 240px/13% 并继续跟随指针。
    实测 `getComputedStyle(.drop-overlay)` 的 `background: none / rgba(0,0,0,0)`、`::before` 不存在，
    卡片实测 340×153。§3.1 已同步为当前设计。
- 2026-09-16：定案为**整窗覆盖卡片**（第三次几何修正，当前设计）。
  - 澄清：前两条反馈是**需求**（“虚框贴着整个 app 边缘”“卡片大到接近 app 窗口，但要有一定透明度”），
    不是缺陷报告；前两版按“遮挡过多”理解反了方向，已回退。
  - 结果：`.drop-overlay__card` 从居中紧凑卡片改为 `position: absolute; inset: 9px` 的整窗面板
    （`border-radius: 15px`、1.5px `--accent` 56% 虚线、`--panel-raised` 74% + `blur(10px)`），
    图标/标题/说明/chip 在面板内垂直居中；`.drop-overlay` 的 `z-index` 提到 10000（高于
    `.debug-queue-launcher` 的 9999，避免 DEBUG QUEUES 按钮压在卡片上）；光晕加大到 320px/26%，
    经毛玻璃面板衰减为柔和移动光斑，`over` 仍然零重渲染。
  - 实测（1280×577 视口）：卡片 1262×559 @ (9,9)，`border: 1px dashed rgba(216,255,104,.56)`、
    `background: rgba(22,25,29,.74)`、`backdrop-filter: blur(10px) saturate(1.05)`；
    `check` / `test`（330 通过） / `build` 全绿。§3.1 已同步。
- 2026-09-16：移除跟随指针的光晕（第四次修正，当前设计）。
  - 需求：卡片铺满整窗后，不再需要沿鼠标移动的光晕。
  - 结果：删除 `.drop-overlay__halo` 元素与规则、`useFolderDrop` 的 `overlayRef` 参数与
    `--drop-x/--drop-y` 写入、`App.tsx` 的 `dropOverlayRef`；`over` 事件改为显式忽略（仍保证
    零重渲染）。`useFolderDrop(onDrop)` 现在只维护 `visible / folderNames / itemCount`，
    `ImportOverlay` 不再接收 ref。
  - 实测（1280×577 视口）：`.drop-overlay__halo` 已不在 DOM 中，卡片仍为 1262×559 @ (9,9)；
    `check` / `test`（330 通过） / `build` 全绿。§3.1 已同步。
- 2026-09-16：去掉毛玻璃（第五次修正，当前设计）。
  - 需求：投放卡片不要 `backdrop-filter` 模糊。
  - 结果：`.drop-overlay__card` 删除 `backdrop-filter: blur(10px) saturate(1.05)`，面板改为
    `--panel-raised` 80% 的纯半透明填充（比带模糊时 74% 略实，用来补偿失去模糊后的背景干扰；
    透明度仍是唯一的“透出程度”旋钮）。整窗覆盖、9px 内缩虚框、`z-index: 10000`、无光晕均不变。
  - 实测（1280×577 视口）：`backdropFilter: none`、`background: rgba(22,25,29,.8)`、
    `border: 1px dashed`；`check` / `test`（330 通过） / `build` 全绿。§3.1 已同步。
- 2026-09-16：导入结果改为底栏提示（第六次修正，当前设计）。
  - 需求：导入**完成后**的提示要和应用其它提示一样落在底栏，而不是留在右下角浮层。
  - 结果：
    - `folderImportSummary` 用 `imported` / `duplicate` 取代原 `settled`，新增
      `failedImportEntries` 与 `applyEntryRetried`（失败行重试成功后回到
      `scanningDirectories` 并清空错误，`applyEntryOpened` 不会复活失败行）。
    - `App.tsx` 新增 `StatusNotice` 类型与 `importNoticeDetail`，把导入结果并进既有的
      `notice` 三级优先级（显式错误 > 导入结果 > 浏览状态）；新增
      `importSummaryFailed` 文案与 `.statusbar__notice-action`（展开面板里的“重试”，一次重试所有
      失败行）；`dismissStatusNotice` 同时清空导入状态。
    - 干净结果 6 秒后自动退场（`IMPORT_NOTICE_MS`）；失败/整次失败为 `kind: "error"`，保留到用户关闭。
    - `ImportPanel` 降级为“仅进行中”：`App.tsx` 用 `folderImport.visible && summary.pending` 门控，
      头部固定为“正在导入 N 个文件夹”，结算后的标题分支已删除。
  - 实测（1280×577 视口，demo drop 两个文件夹）：结算后 `.import-panel` 不在 DOM，
    `.statusbar__notice--status` 文本为“已导入 2 个文件夹 · 0 张照片”，7 秒后提示自动消失；
    `check` / `test`（331 通过） / `build` 全绿。§3.2 / §3.3 已同步。
- 2026-09-16：`+` 按钮与拖拽共用导入管线（第七次修正，当前设计）。
  - 需求：用 `+` 按钮（文件选择器）导入要和拖拽导入有一致的效果。
  - 结果：`App.tsx` 把 drop 专用的 `handleDropPaths` 重命名为 `startFolderImport` 并前置，
    `handleOpen` 改为 `chooseFolder()` 取路径后调用同一条管线；原 `openPath` 保留但只服务性能
    harness（不注册根、不产生导入提示）。选择器仍是单选，语义未变，变的只是反馈。
  - 实测（1280×577 视口，demo 点击侧边栏 `+`）：底栏出现“已导入 1 个文件夹 · 0 张照片”，
    侧边栏出现 `Field Notes` 及其子目录并开始浏览；结算后无右下角浮层。
    `check` / `test`（331 通过） / `build` 全绿。§3.3 已补“入口一致”一条。
- 2026-09-16：删除浮层信息面板，导入全过程都在底栏（第八次修正，当前设计）。
  - 需求：不要信息面板，导入**整个过程**（进行中 + 结果）的提示都放在底栏。
  - 结果：
    - 删除 `components/ImportPanel.tsx` 与其测试，`styles/import.css` 只保留拖入卡片样式
      （`.import-panel*` / `.import-row*` 规则与两个 keyframes 全部移除）。
    - 新增纯模块 `lib/folderImportNotice.ts`：`folderImportNotice(state, t)` 同时覆盖进行中与已结算，
      产出 `{ kind, message, detail, retry }`；`folderImportDetail` 生成逐根明细（每根一行
      `名称 — 阶段`，外加被忽略计数与不可访问路径）。`App.tsx` 里的 `importNoticeDetail` 随之删除。
    - 进行中文案为“正在导入 N 个文件夹[ · NN%]”，百分比取当前工作行（索引队列串行，因此唯一）；
      明细放在既有 `statusbar__notice-panel` 里，该面板补 `max-height: 42vh; overflow-y: auto`。
    - 新增 `lib/folderImportNotice.test.ts`（9 个用例，覆盖进行中/百分比/结算/部分失败/整次失败/
      全重复/全忽略/不可访问路径明细）。
  - 实测（1280×577 视口，demo drop 两个文件夹）：浮层不存在；底栏为“已导入 2 个文件夹 · 0 张照片”，
    点开后明细两行“2024-北海道 — 完成 · 0 张照片 / Weddings — 完成 · 0 张照片”；
    `check` / `test`（337 通过） / `build` 全绿。§3.1–§3.4 已同步。
- 2026-09-16：去掉“多个文件夹之间关系”的判断与解析步骤（第九次修正，当前设计）。
  - 需求：不用检查多个文件夹的关系，也不需要比原来“打开文件夹”多出任何操作。
  - 结果：删除 `resolve_drop_paths` 命令、`oxy-fs::resolve_dropped_roots` 与
    `DroppedPathsResolution` / `DroppedPathRejection` / `DroppedPathRejectionReason` 契约；前端删除
    `resolveDropPaths`、`listLibraryRoots` 比对与“已在图库中”判定、`applyDropResolution`；
    导入回到 `open_folder` + `add_library_root`，只是加上逐行记账与底栏提示。
    失败改由 `open_folder` 的返回值决定（普通文件也会逐行失败，不再被静默忽略）。
- 2026-09-16：去掉索引进度解析（第十次修正，当前设计）。
  - 需求：不用解析进度。
  - 结果：删除 `library-index-progress` 事件、`LibraryIndexProgress` / `LibraryIndexStage` 契约、
    `LibraryIndexQueue` 的节流播报（`should_report` / `clear_reported` / `PROGRESS_REPORT_INTERVAL`
    与其单测），`update_progress` 回到只更新队列快照；前端删除 `onLibraryIndexProgress`、
    `applyIndexProgress`、`importPercent` 与两个进度文案，`FolderImportEntry` 只剩 `assetCount`
    （由既有 `library-index-updated` 完成事件提供）。阶段收敛为
    `registering → indexing → ready | failed`，底栏不再出现百分比与逐目录计数。
- 2026-09-16：范围澄清（第十一次，当前设计）。
  - 需求：只需要 `+` 按钮与拖拽的后续逻辑一致，不再追加任何额外调整。
  - 结论：`startFolderImport(paths)` 是唯一导入实现，`handleOpen` 与 `useFolderDrop` 都只调用它，
    管线内没有任何按钮专属或拖拽专属分支；**明确不做**多选选择器、重复/嵌套判定、实时进度、
    软撤销等附加项。拖入卡片是拖拽特有的视觉提示，不属于后续逻辑。
