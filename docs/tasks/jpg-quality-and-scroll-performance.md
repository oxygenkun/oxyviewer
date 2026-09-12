# JPEG 质量分层与滚动卡顿实施计划

日期：2026-09-12。状态：**S0–S6 实施与验收完成；未达预算项单独保留**。

结果见[Windows 实测报告](../research/jpeg-thumbnail-scroll-2026-09-12.md)。滚动主线程长任务已消除，
但仍有启动／GPU 长帧、Sony 暖启动单次尾部和既有十万文件首屏预算缺口；不能解读为所有性能目标均达标。

主设计：[按源表示规划 thumbnail 与 full](../architecture/08-representation-planning.md)。
设计已进入生产代码实施；所有阶段完成以实际代码和验收记录为准。

## 1. 已有证据与优先级

本会话使用 Windows 现有 Release、隔离 data/cache/WebView profile，核查两个真实 NAS
RAW+JPEG 子目录；未清空 OS/SMB cache，没有将这些样本称作严格冷盘或跨平台基准。

| 项目 | 当前证据 | 后续处理 |
| --- | --- | --- |
| 大 JPEG → 主线程 Canvas.toBlob | Sony 同步调用约 362–478 ms；Lumix 4000×6000 为 218–246 ms；trace 内含主线程 JPEG Decode Image | 首要修复，S1–S3 |
| JPEG 已含可用小图 | Sony 241/241、Lumix 69/69 均有 IFD1 160×120 和 MPF 约 1600×1080 | 先按偏移提取，用内嵌图生成最终 thumbnail |
| 大原图传输 | JPG 平均约 19.4 / 11.9 MiB；协议会完整 materialize 原文件 | S1–S2 减少 thumbnail 输入；full 的原图协议不是本轮流式重构目标 |
| RAW 内嵌图仍需缩小 | 1616×1080 / 1920×1280 表示进入浏览器 Canvas；Lumix 同类同步成本约 29–33 ms | S2 接入交付约束，full 策略独立 |
| grid/filmstrip 渲染放大 | Loupe 内 virtualizer、未 memo item、变化 props、flushSync 调用链 | S4 定量后修复 |
| 元数据使整目录重新映射 | App 订阅 records，projectAssetMetadata 产生新对象；preload 重新遍历 | S5 稳定引用与订阅 |
| 标签查询与 native 同步 DB command | filmstrip 按项查询，get_asset_tag_assignments 同步 | S5 批处理、移出 UI 线程，独立测量 |
| 并发/内存/调度延迟 | browser preload 按 hardwareConcurrency；不同消费路径会叠加 | S3 加有界预算，保留原优先级及取消语义 |
| 原生锁竞争/诊断 | 一次预热后队列全 idle、采集 24μs；未证明预热中无锁竞争 | S0/S6 分阶段测量，不先重写原生队列 |

这次观测中，Lumix grid 预热中最大帧间隔 274.9 ms、预热后 6.5 ms；filmstrip 预热后
49.9 ms，页面重载后 225 ms。Sony 脚本 filmstrip 达 450 ms。
Lumix 使用 CDP 滚轮且 isTrusted=true；Sony 用脚本位置变化。不能混成同一种输入统计。
P95 可很低而仍有几百毫秒停顿，因此必须记录最大帧间隔和 >50 ms 长任务。

同图 ImageBitmap 预缩小实验将 toBlob 同步部分从约 215 ms 降至 44/2 ms，但首次最大帧间隔
仍达 81 ms，且未做画质校验；它只支持方向选择，不是已验证的生产修复。
早期包装不可写的 Tauri invoke 得到空 calls，已排除，不能以此宣称没有 IPC。

原始测量在 ignored 的 `tests/perf/.reports/nas-scroll-20260912/`，包含原始 trace、wheel JSON、
内嵌图 inventory 和提取样本；本计划仅保留去除机器/NAS 路径的摘要，不提交私人照片。

## 2. 顺序与完成状态

```text
设计完成 → S0 基线/fixture → S1 JPEG planner 与执行
                         → S2 缓存/浏览器与 RAW 对齐
                         → S3 剩余转换/预算
                         → S4 滚动组件 → S5 元数据/标签 → S6 最终验收
```

S1/S2 是一个完整的用户可见交付单元：不能仅合并 planner 后宣称卡顿修好。
S4/S5 在 S2 后重复基线，分别测剩余成本，避免把 S1 的收益重复计算。

- [x] 核查现有语义 planner、native dispatcher、缓存替代、投影、浏览器保留。
- [x] 完成可扩展 JPEG thumbnail/full 设计与性能实施计划。
- [x] S0：建立可重复基线和解析 fixture。
- [x] S1：实现 JPEG 表示探测、planner、执行及回退。
- [x] S2：接通缓存/projection/浏览器保留，RAW thumbnail 交付对齐。
- [x] S3：消除剩余主线程大图转换，建立转换预算。
- [x] S4：收敛 grid/filmstrip 的滚动渲染开销。
- [x] S5：收敛元数据更新与标签查询。
- [x] S6：两组真实目录、跨格式与资源生命周期验收，记录通过项及未达预算项。

## 3. S0：基线与 fixture

交付：将本会话一次性探针整理为可重复 runner，测试图片仍通过外部 fixture 路径提供。
先记录新构建对应的 revision、frontend hash、窗口尺寸、DPR/刷新率、目录规模和缓存状态。

- parser 单元测试用合成 JPEG APP1/APP2/MPF，覆盖大小端与相对 offset，避免把 ExifTool 输出当规范。
- 真实 Sony/Lumix JPG 仅用于外部 fixture 配对检查；覆盖相应 RAW，另备无 EXIF/无 MPF/已编辑 JPEG。
- 采集 native request → probe/cache → 提取 → decode/resize/encode → registry → WebView fetch/decode → 实际显示。
- 浏览器记录 rAF 分布、长任务、wheel 接收到 scroll 位移/下一帧、可见图片 ready 与实际绘制证据；
  宿主发送时间与浏览器时间需要校准，不把两种时钟直接相减。
- 区分 OS wheel（可用时）、CDP wheel 与脚本 jump；确认前台可见、isTrusted 和探针确实运行。
- IPC 计数使用已验证的可观测边界/原生诊断；不要修改只读属性后静默得到零。

通过条件：同一构建能重复观察预热中的大图转换长任务与预热后对照，采样本身没有明显阻塞。

## 4. S1：JPEG 表示 planner 与执行

修改范围：`oxy-metadata-parser`、`oxy-media/formats`、`delivery.rs`、`pipeline/jpeg/selection.rs`、
`pipeline/jpeg.rs`、必要 backend/presentation 小函数。公共变更才进入 oxy-domain。

具体工作：

1. 有界读取 EXIF/MPF/SOF/方向/色彩，输出有源版本、角色、区间与几何的候选。
2. 实现纯 planner 的质量与 DeliveryLimits 校验；普通小 JPG 直接交付，大 JPG 优先合格内嵌图。
3. native 从 MPF 范围提取、缩小到 thumbnail 交付预算；没有合格预览时 native 主图缩小。
4. full 继续使用主图，保持已有 RAW full 定义；不新增高质量 JPEG AssetKind 或 vendor UI 分支。
5. 候选损坏/unsupported 与取消/stale/I/O 分开；对失败候选有限次回退，不重试整个列表。

验证：

- 纯 planner 矩阵覆盖：小原图、大原图+MPF、多个有效预览、只有160、无预览、不同vendor、非法角色、
  geometry/ICC 不可用、已有高清 cache、所有 backend 失败；分别检查 thumbnail 与 full。
- MPF 越界/溢出/自引用/截断、错误关联、stereo/gainmap、预算超限；读取前后源版本变化。
- 实际源 seek 区间计数；有效 MPF 路径不读取主图压缩像素段，不能仅统计最终输出小图字节。
- 8 种 EXIF 方向、黑边 contentRect、主图/内嵌图 ICC，配对原图与 thumbnail 截图。

不通过颜色/内容对应验证的候选走安全的主图转换；不得为通过性能指标而忽略呈现差异。

## 5. S2：缓存与端到端交付；RAW 对齐

修改范围：`oxy-media/cache`、`policy.rs`、Tauri preview 恢复/发布边界、`Thumbnail.tsx`、
`api.ts`、`folderThumbnailCache.ts`、对应测试。

- JPEG thumbnail 独立 policy revision；查内存投影、SQLite 恢复和磁盘 artifact 三条快路径都验证
  当前策略与交付上限。旧 original thumbnail projection 不能直接命中。
- 保留 larger cache 作为派生输入的能力；禁止超限 artifact 直接作为 thumbnail 浏览器资源。
- 统一小 Blob 的加载、解码、保留与按源去重；可见 onload 和后台 preload 不再重复转换。
- RAW thumbnail 内嵌大 JPEG 接入同一小图规范化；只调整 thumbnail 对应策略，full/preview 若共用
  版本常量需显式拆分，不顺带改变最大相机 JPEG 的 full 选择。
- native 原样提取的小图和重新编码的派生图分清 provenance；继续复用 v2 缓存与 lease。
- 本阶段不做 JPEG 160 interim 升级，避免低清保留阻止最终清晰图替换。

验证：旧 projection、损坏 artifact、高清 cache 命中、clear 期间派生、同时请求同图、切目录、
失效后晚到 fetch、虚拟卸载/重挂、超过通用 LRU 容量、native lease 释放和 browser Blob 清理。
JPG/RAW 冷 thumbnail 都不得把超限图传给主线程；full 仍保持完整显示细节。

通过条件：两个真实目录普通 grid/filmstrip 只接收有界小图；全目录预热最终完整保留；
热滚动不新增 native thumbnail 请求、转换或原图读取，并由真实显示验证。

## 6. S3：剩余主线程转换与预算

先检查 S2 后 JPEG/RAW 正常路径是否已经不需要浏览器 Canvas。保留已有 HEIF 小图直接路径。
对于未接入 planner 的大 PNG/WebP/TIFF 或其他必要 fallback，实施独立的有界转换入口：

- 优先原生派生小图；需要浏览器 fallback 时使用专用 Worker + ImageBitmap/OffscreenCanvas。
  以 bundler 管理的同源 worker 文件加载，核对打包 CSP，不依赖随意放开 blob worker 来源。
- feature 不支持时返回原生转换路径；没有可用非阻塞路径则明确失败，不恢复主线程大图 toBlob。
- 一个任务以源版本+输出策略去重，加入新可见消费者要能提升尚未开始的任务；不将 query/native
  排队数量当作浏览器解码数量。无需再建立一套不互通的全局优先级语义。
- 初始转换并发上限 2、临时转换预算 256 MiB，作为可测的起始配置而非硬件普适最优值。
  原生与 Worker 各自执行时使用明确分配的预算，不能把两个独立的 256 MiB 宣称为全局256。
  大主图可独占许可，小预览可并行；预算覆盖输入解码、输出及已知后端 scratch，超过预算采用
  支持缩放解码的 backend 或明确拒绝，不能按输出512估算输入成本。
- 等待预算必须有优先级和取消；不在持有全局锁时等待。已运行任务的取消能力按 backend 如实记录。
- 整目录 background 保留最低优先级和有界 in-flight；保留完成的缩略图，不因滚动驱逐。

验证：转换队列完成/失败/取消都归还许可；Worker 被终止/不支持/迟到输出释放 bitmap、Blob URL；
大量独立照片下观测 native、WebView renderer、GPU 子进程工作集，分别报告估计和实测。

## 7. S4：grid 与 filmstrip 渲染

- 将 filmstrip virtualizer、可见范围和 item 子树从 Loupe 主图/缩放状态拆出，memo item，稳定
  选择/菜单回调与布局值；保留 active selection、分页、右键菜单及尺寸调整行为。
- grid 已有 memo 和稳定回调，先测因 rank/priority/metadata 对象改变而造成的 render，
  不重复把既有修复写成新收益。
- 可见集合/离散排序变化时才 reconcile intent；合并同帧变化，保留最新 viewport、scope epoch、
  in-flight transport 后的最新更新和 release，不能让被合并更新丢失取消或提级。
- wheel 统一 pixel/line/page 的 deltaMode，处理原生横向输入，合并每帧位移并保持方向/总距离；
  如需 preventDefault，用明确的可取消监听策略，避免重复执行浏览器默认滚动。
- 评估缩小 overscan、`useFlushSync` 或 direct DOM 更新时一次只变一个变量；不能用减 overscan
  换取快速滚动空白。优先缩小 React 更新边界，保留虚拟列表的同步正确性。

验证：正反滚轮、快速远跳、250-item 分页边界、目录末尾、loupe 当前图不随滚动重置、选择自动揭示、
filmstrip resize、列表/网格切换；记录提交次数/耗时和可见空白，不只跑 React 单元测试。

## 8. S5：元数据与标签

- 对未改变 rating/colorLabel/pickLabel 的投影保留 AssetSummary 引用；相同源摘要和同样显示字段
  才可复用，不能覆盖新分页摘要或源变化。
- 可见卡片按资产订阅显示字段；筛选/排序需要全局变化时单独做增量/批量更新，不能丢掉筛选正确性。
- 合并同一帧/有界短批次的 projection 通知，保留 stateRevision/validAt 单调性；不延迟用户主动
  评级到不可感知性标准之外。对 preload 使用稳定源身份列表，减少 metadata-only 更新触发重扫。
- 可见标签改为一批查询并分发到按路径缓存；补齐 assign/unassign/重命名等操作的失效和更新。
- `get_asset_tag_assignments` 的数据库读移至后台，Tauri 保持薄；不在其他全局锁内等待数据库。

验证：未变化元数据不重渲染整目录；评级和标签实时更新、筛选/排序结果正确；并发批次不会把旧结果
覆盖新修改；数据库忙时滚轮/窗口仍能响应。测量确认剩余成本下降再宣称改善。

## 9. S6：最终验收矩阵

每个阶段保留原基线，最终在新构建的真实 Release/WebView 中对两组独立照片至少重复三轮。

| 维度 | 必测场景 |
| --- | --- |
| 缓存 | 独立应用冷缓存；磁盘小图热/浏览器冷；全目录浏览器热；重开进程；不清 OS cache 的事实 |
| 交互 | 打开首屏；预热期间 grid/filmstrip 滚轮；反向；快速 jump；底部；选中 JPEG/RAW full |
| 正确性 | 当前完整主图、缩略图方向/ICC/contentRect、全目录保留、选择恢复、无上一张残影 |
| 并发 | 后台预热+可见请求；源变化；切目录；取消；clear；晚到发布；旧投影恢复 |
| 扩展 | 无 MPF/坏 MPF/未知vendor/编辑后 JPG、小 JPG、RAW、现有 HIF、未迁移格式 fallback |

同时报告：首屏绘制、native thumbnail、浏览器首图/全视口 ready、全目录解码保留时刻、
>50 ms 长任务数量/总时长/最大值、帧间隔 P50/P95/max、输入到位移、读取字节/次数、
IPC 延迟分段、进程及子进程峰值内存。不能用低 P95 掩盖少量 400 ms 停顿。

性能门槛继续采用 [PERFORMANCE.md](../PERFORMANCE.md)：主线程滚动不得出现 >50 ms 长任务；
本地 SSD 首屏300ms/热loupe150ms/冷选中800ms独立测量。NAS单列结果，不挪用本地预算作已达标结论。
三轮最大值不能称为统计稳定的尾延迟；预热完成速度不能代替滚动/首屏验收。

若仍有长任务，按 trace 的具体调用归因处理；不能通过过滤样本、调低画质或放宽预算结束任务。
每轮关闭自有测试实例；文档去除私人机器和 NAS 路径，照片 fixture 不入库。

## 10. 检查与交付边界

当前已进入实现验收，文档需要与生产代码和实际测试结果保持一致。

后续实现按仓库约定：

- Rust：`cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`。
- 前端：`pnpm check`、`pnpm test`、`pnpm build`。
- 跨层阶段两套都跑；fixture 测试必须确认实际执行，不把跳过当通过。
- 每个阶段记录改动文件、检查、真实界面结果与未解决限制。文档和实现状态分别更新。
- 其他格式推广、通用渐进160底图、全图协议流式化均需各自证据，不能借本次方案自动改掉既有行为。
