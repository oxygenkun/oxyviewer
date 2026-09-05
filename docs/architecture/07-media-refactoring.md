# 07：oxy-media 重构规则与目标

状态：分阶段实施。本文描述目标架构，不代表所有模块和行为已经实现。
当前流水线见 [03](03-preview-pipeline.md)，会话见 [04](04-heif-tile-session.md)。

## 1. 目标与问题

支持 Windows/macOS 上不同 RAW 变体和不同厂商 HEIF/HIF，同时保留已有 Linux 路径。
按职责而非“平台 × 格式 × 厂商”复制流水线：新增平台主要增加后端，新增文件变体主要增加
探测、兼容规则和 fixture，不重复缓存、调度与 fallback。

当前 `lib.rs` 混合 API、路由、锁、缓存和编码；`heif.rs` 实际封装 libheif 并处理色彩；
`embedded_jpeg.rs` 实际是 Sony SHIF 提取器；preview 和 HEIF session 分别选择后端。
Sony 160px 快速表示目前被应用于所有 HEIF 的 thumbnail/preview，后续应由文件事实决定，
而不是继续扩展这种格式级特判。

## 2. 不可破坏的规则

1. 目录打开保持廉价、非递归、分页。源探测按需且有界，不为路由同步读取完整 EXIF 或解码。
2. 保留语义 RenderLevel、优先级、pending 去重和取消边界。运行中不可抢占不等于可取消；
   RAW full 独立 lane 不得在机械拆分时并入统一 gate。
3. 不通过 JSON IPC 传图片字节。保持路径/受控协议和可重建缓存边界。
4. 平台 `cfg` 收敛到后端注册和适配层；是否真正支持某个文件由运行时能力和实际解码决定。
5. 厂商只是兼容性线索。优先按容器、codec、位深、采样、grid、方向等特征处理；
   只有真实差异才添加厂商规则，不为目录对称预建空模块。
6. 厂商规则不写缓存、不持有全局锁、不调度任务。后端不决定 UI 等级或完整 fallback 链。
7. 方向、色彩和表示来源必须显式：区分未应用/已应用方向、ICC/NCLX、SDR/HDR；
   全尺寸相机 JPEG 不等于 RAW 显影结果，尺寸更大不自动代表缓存可替代。
8. 不把完整解码后切块交付报告为原生 tile 解码，不把原生 API 报告为已证明的硬件加速。
9. 取消应终止计划，不触发 fallback；区分不支持、不可用、损坏、IO 和取消，保留尝试诊断。
10. 公共序列化契约放 `oxy-domain`；通用作业语义复用 `oxy-runtime`；Tauri 保持薄。
11. 先在一个 crate 内整理，避免提前引入插件框架、巨型 decoder trait 或每厂商一个 crate。
    Rust 模块用 `foo.rs` + `foo/`，不用 `mod.rs`，遵循 [Rust 风格](../RUST_STYLE.md)。

## 3. 目标职责与依赖

```text
lib.rs                      稳定公开入口与重导出
service.rs                  请求编排
probe.rs + probe/           SourceFacts：有界、分阶段识别源文件
formats.rs + formats/       RAW/HEIF 格式知识、items、局部 quirks
backends.rs + backends/     LibRaw/libheif/FFmpeg/ImageIO/WIC 适配与能力
pipeline.rs + pipeline/     planner、executor、session
decode_control.rs           媒体解码 gate、独立 lane 锁与同源锁表
presentation.rs + presentation/  方向、色彩、缩放
cache.rs + cache/           key、store、encode
```

目录按实际迁移逐步建立。FFI 源文件和 build.rs 路径需一起迁移。

```text
Request + SourceFacts + BackendCapabilities
                  ↓
              DecodePlan
                  ↓
       缓存查询 / executor / session
                  ↓
       后端 → 显示规范化 → 缓存或协议交付
```

- `SourceFacts` 描述格式族、具体变体、可用表示、尺寸、方向与色彩；未知信息保留未知。
- Request 描述等级、目标尺寸、优先级、呈现意图和取消信号，而非指定解码库。
- 能力描述特定输入支持、缩放/full/tile、输出色彩与方向状态、取消和加速证据。
- planner 使用事实与能力生成可测试的计划，不执行 IO/解码。探测由编排层按需完成。
- preview 和 full session 使用相同选择机制，但允许不同计划和资源 lane。
- fallback 顺序用兼容性与性能证据决定，不假设原生后端永远最好。
- 缓存替代要比较表示、质量和处理策略；影响产物的行为变更需更新 cache version。

## 4. 迁移阶段与验收

每次只做可独立检查的小步。机械迁移和行为变更分开；旧 API 用重导出或薄 wrapper 保持兼容。

### A：机械拆分（已完成本阶段清单）

- [x] 提取缓存容量管理至 `cache/store.rs`，移动就近测试并保持根模块重导出。
- [x] 提取通用缓存键与编码，各自移动就近测试（格式专用源转换仍留在原路径）。
- [x] 提取解码 gate 与锁的所有权；不顺便改变并发度或锁顺序。
- [x] 将 libheif 适配器和 Sony 专用提取器命名对齐职责，整理后端与 FFI。
- 验收：API、产物、缓存键、fallback 顺序、锁范围不变；原有测试通过。

### B：事实与计划

- [x] 引入最小内部 SourceFacts / DecodePlan，先表达当前行为。
- [x] planner 用模拟能力测试平台 × 文件特征 × 等级矩阵，不依赖宿主原生解码器。
- 验收：未知厂商、能力缺失与现有策略都有明确结果；探测不进入目录首屏。

### C：统一后端选择（已完成本阶段清单）

- [x] preview/session 共用能力判断与 fallback 机制。
- [x] 明确错误分类、尝试诊断、取消短路与迟到结果处理。
- 验收：后端失败、不可用、取消、session 生命周期均有回归测试。

### D：修正策略与产物契约

- [ ] 将 Sony 160px 从全 HEIF 默认策略改为经识别的快速表示。
- [ ] 明确 RAW 相机预览与显影意图、方向/色彩状态、缓存替代条件与版本。
- 验收：真实 fixtures 覆盖方向、色彩、尺寸、未知厂商、冷/热缓存及失败回退；
  依照 [性能预算](../PERFORMANCE.md) 做同平台、同 fixture、同构建条件比较。
  涉及 renderer/IPC 时同步前端、domain、demo 与对应测试。

## 5. 验证和进度记录

每一步运行 `cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、
`cargo test --workspace`。跨边界变更再运行前端检查。平台特有代码需对应平台 CI/真机验证，
本机通过不等于跨平台通过；没有基准不得声称性能不退化。

第一小步已实现：仅提取缓存容量统计、清理和裁剪至 `cache/store.rs`，保留根模块公开 API；
缓存键、编码、解码调度和格式策略暂不改动。迁移原有两项测试，新增缺失目录、非递归统计、
受保护产物超预算三项测试。

本机 macOS 验证：

- `cargo fmt --all --check`：通过。
- `cargo clippy -p oxy-media --all-targets -- -D warnings`：通过。
- `cargo test --workspace`：通过（含项目已有 ignored 测试，不代表所有 fixture 基准已执行）。
- workspace Clippy：被未修改的 `crates/oxy-fs/src/lib.rs:698` 中 `path` 未使用告警阻挡，
  本步不附带修复无关代码。
- 未运行 Windows/Linux 原生验证或性能基准；本步未启动调试实例。

第二小步已实现：

- `cache/key.rs` 接管 canonical path、大小、mtime、backend tag 与尺寸的哈希，算法和顺序不变。
- `cache/encode.rs` 接管通用 JPEG/ICC 编码、计时写入、字节写入与原子提交。
  macOS 计时路径继续使用 ImageIO，Windows/Linux 继续使用 image encoder；已有文件不覆盖。
- 根模块保留调用位置，cache version、锁范围和释放时机未改。
  Quick Look 和 HEIF 源转换仍留在原路径，未借机统一其不同写入语义。
- 迁移两项 key 测试，新增七项测试，覆盖源身份/大小/mtime、缺失源、ICC、JPEG 可解码性、
  不覆盖已有产物、失败提交清理和缺失父目录。
- macOS：格式检查、oxy-media Clippy、14 项缓存测试、workspace 测试通过。
  workspace Clippy 复查仍被上述 oxy-fs 未使用变量告警阻挡。
- 未进行 Windows/Linux 原生验证或性能基准，未启动调试实例。

第三小步已实现：

- `decode_control.rs` 持有统一 DecodeGate、RAW full 独立锁、HEIF session 缓存写锁和同源锁表。
- 根模块继续导出 DecodePriority/HeifDecodePriority 和内部兼容入口；RAW full 在原位置
  通过薄 helper 获取同一独立锁，不改变 guard 生命周期、锁顺序或并发度。
- 迁移两项优先级测试并增加不可抢占断言；新增 permit unwind 释放、同源锁身份/不同源独立、
  源解码 panic 后 poisoned lock 恢复三项测试。
- 原有同源锁表不回收及其 unsafe lifetime 实现原样保留，不应在未重新设计所有权前删除表项。
  本步只集中所有权，不宣称已解决长期锁表增长或提供运行中取消。
- macOS：格式检查、oxy-media Clippy、5 项 decode_control 测试、workspace 测试通过。
  workspace Clippy 仍被上述 oxy-fs 告警阻挡。未运行其他平台原生验证或性能基准，未启动调试实例。

第四小步已实现：

- LibRaw、libheif、FFmpeg、ImageIO、WIC 迁入 `backends/`；原 `heif.rs` 明确命名为
  `backends/libheif.rs`，不再与 HEIF 格式知识混名。
- 原 `embedded_jpeg.rs` 迁为 `formats/heif/quirks/sony.rs`，生产算法不变。
  新增非 Sony 输入与缺少 JPEG 的失败测试；真实 fixture 使用 manifest 绝对路径，缺失不静默跳过。
- C/C++ wrapper 分别迁到 `backends/apple_image_io/wrapper.c` 和 `backends/libraw/wrapper.cpp`，
  `build.rs` 同步编译和 rerun-if-changed 路径，native 符号、编译参数和依赖不变。
- 核对迁移后各适配器：仅模块引用变化，两个 native wrapper 和 libheif/WIC 实现逐字不变。
- 根模块保留部分私有 backend imports 作为渐进迁移的调用路径；没有新增公开 API。
  libheif 内的色彩/编码职责、ImageIO 内的跨后端 fallback、session/preview 的重复选择逻辑
  尚未拆开，不能把目录整理视为目标架构已全部实现。
- macOS：格式检查、oxy-media Clippy、workspace 测试通过（媒体 59 passed、8 ignored，含 Sony 4 项）。
  workspace Clippy 仍被上述 oxy-fs 告警阻挡。未验证 Windows/Linux native build 或性能基准，
  未启动调试实例。

A 阶段后 filmstrip 回归排查：

- 用户报告切换选中项后上一项缩略图消失。组件测试复现：缓存已 ready 的图片直接显示，
  但没有走 pending 图片的 onLoad，因此未记录 displayedImage；projection 更新清除共享缓存后，
  同一个 DOM 图片会退回隐藏的 pending 状态，src 不变时不能依赖再次 load 来恢复显示。
- `Thumbnail` 在 layout effect 中保留已经采用的 ready 图片，不再让共享缓存淘汰决定
  当前已挂载图片是否可见。保留 asset ID 校验，禁止把上一张图片带到另一资产。
- 添加 jsdom 组件测试（StrictMode），覆盖 projection 更新后取消选中、缓存淘汰并暂停加载、
  冷图加载后提权/降权、切换资产。修复前前两项失败，修复后通过。
- 这是已复现的前端显示生命周期缺陷；没有证据证明是 Rust 文件迁移引入，也未宣称已经完成
  用户桌面环境的连续切图实测。未改变后端调度和共享缓存失效策略。
- `pnpm check`、`pnpm test`、`pnpm build` 通过；未启动调试实例。

A 阶段后快速滚动缓存清理告警修复：

- `cache/store.rs` 原先在目录枚举后直接 `entry.metadata()?`。并发原子提交重命名临时文件，
  或其他清理删除文件，都会使刚列出的条目在读取属性时返回 NotFound。
- 目录打开、迭代条目和属性读取仅容忍 NotFound；删除原有 exists 前置检查，其他 IO 错误继续返回。
  不增加解码锁，不改变删除范围或原子写入协议；统计仍是近似快照。
- 新增三项测试：Unix 上确定性地在枚举后提交临时文件、迭代 NotFound/真实错误分类、
  缺失目录与非目录路径。Windows DirEntry 可能缓存属性，因此该实际重命名竞态测试限 Unix。
- macOS 格式检查、oxy-media Clippy、8 项 store 测试、workspace 测试通过；workspace Clippy
  仍受已有 oxy-fs 告警阻挡。未启动调试实例或执行真实快速滚动性能测试。

B 阶段已实现：

- 新增内部 `pipeline/planner.rs`，以最小 `SourceFacts`、`BackendCapabilities`、请求和
  `DecodePlan` 表达原有 preview 路由；planner 为纯函数，不接收路径且不执行文件、缓存或解码 IO。
- `preview` 使用未探测事实和原有乐观路由能力生成计划，再由原执行函数完成工作。RAW preview 的
  system fallback 错误组合、HEIF full 的前台 8192 fallback、RAW full 独立 lane、缓存键、锁范围和
  公开 API 均保持不变；HEIF session 选择仍留在原处，等待 C 阶段统一。
- HEIF thumbnail/preview 继续对所有厂商请求 160px。未探测和未知厂商仍尝试原有有界 Sony
  表示提取，已知缺失表示时计划可跳过；没有把 D 阶段的策略修正提前带入。
- host-independent 单元测试覆盖 3 个平台 × 6 种 `AssetKind` × 3 个 `RenderLevel` 的现有矩阵，
  并交叉覆盖 HEIF 厂商/快速表示事实，以及 RAW、HEIF、TIFF 能力缺失时的 fallback 或
  `Unsupported` 结果。测试直接注入能力，不调用宿主原生能力或 fixture。
- 本步没有向目录打开或首屏发现加入探测；生产 `SourceFacts` 从已有 `AssetKind` 构造，其余事实
  保持 unknown，原 Sony 文件读取仍只在 HEIF preview 请求执行时按需发生。
- macOS：格式检查、oxy-media Clippy、7 项 planner 测试、oxy-media 测试和 workspace 测试通过
  （媒体 66 passed、8 ignored）。workspace Clippy 仍被未修改的
  `crates/oxy-fs/src/lib.rs:698` 的 `path` 未使用告警阻挡。
- 未运行 Windows/Linux 原生构建或真机验证，也未运行性能基准；本步未启动调试实例。

C 阶段已实现：

- 新增 `pipeline/heif.rs`，集中 HEIF preview、full artifact 和 full session 的运行时能力判断、
  输入支持探测、有序候选与统一 attempt executor；纯 selector 使用模拟 probe 测试，不依赖宿主
  decoder。preview 与 session 可以生成不同顺序，但不再各自实现 fallback。
- 保留当前平台策略：macOS preview 优先 ImageIO，Windows preview 优先 FFmpeg；session 在 Windows
  保留 FFmpeg JPEG grid、FFmpeg RGBA、libheif 的既有回退，在 macOS 保留 ImageIO、FFmpeg、
  libheif 顺序。ImageIO full JPEG 适配器内的隐藏 FFmpeg fallback 也移到统一 executor。
- attempt 按 unsupported、unavailable、corrupt、IO、decode failure 和 cancelled 分类。成功 fallback
  写入现有 `fallbackReason`；全失败通过 `BackendAttempts` 保留有序诊断及最终 source error，未新增
  跨 IPC 的序列化契约。
- executor 在每次 attempt 前、不可抢占的 native 调用返回后及 fallback 前检查取消；取消直接终止
  整个计划，迟到成功结果不进入 tile、完成回调或缓存。格式级 RAW/system 与 HEIF full/preview
  fallback 同样对 `Cancelled` 短路。
- session 在 `begin` 时保存 plan；尺寸或规划前置工作完成后才替换 active session，保持失败 begin
  不取消旧 session。tile 与 complete 通过 session ID、generation、token 和 publication 边界校验，
  新 session 不接受旧结果；成功替换仍取消旧 token、清 tile，并同步清理旧 diagnostics。
- host-independent 测试覆盖 preview/session 顺序、不可用/不支持、IO/损坏/解码失败诊断、全失败、
  fallback 成功、取消后不回退及迟到成功丢弃；service 测试覆盖失败 begin、成功替换、旧 tile 拒绝、
  stale cancel 和取消后不 complete。原有真实 HIF fixture 集成测试继续通过。
- 本阶段没有修改 HEIF thumbnail/preview 的全格式 160px 策略、RAW full 独立 lane、缓存 key、
  RenderLevel、解码 gate 并发度或 IPC 图片交付方式；这些策略仍留给 D 阶段。
- macOS：`cargo fmt --all --check`、oxy-media Clippy、`cargo test --workspace` 通过（媒体
  75 passed、8 ignored）。workspace Clippy 仍被未修改的 `crates/oxy-fs/src/lib.rs:698` 中
  `path` 未使用告警阻挡。未运行 Windows/Linux 原生构建或真机验证，也未运行性能基准；
  本阶段未启动调试实例。

下一小步：进入 D 阶段，以按需探测到的表示事实收窄 Sony 160px 快速路径，并明确方向、色彩、
RAW 相机预览/显影意图与缓存替代契约；不要把本阶段内部 attempt 诊断误报为新的 IPC 契约。
