# 媒体缓存与产物发布重构计划

状态：媒体缓存实现与 macOS 本机验收已完成。用户已取消 100k 冷启动 300 ms 的严格完成门禁，保留其参考指标；Windows/Linux 文件锁、rename/sharing 与 packaged WebView 矩阵尚未运行，用户选择暂不提供环境，因此完整跨平台验收未完成。实际浏览发现 resource registry 的统一 64 项/128 MiB 限制会让 HEIF Full 的文件型产物因 entry 耗尽而无法发布；文末追加的资源预算与生命周期修复已实施，并通过 macOS 本机压力验收。本文保留最初设计及按时间追加的实施记录；以本状态和最新记录为准。

## 1. 业务目标与已确认决策

1. 同一张图片可以保留多个清晰度产物。
2. 兼容的高清产物可以满足低清请求，低清不能冒充高清。
3. 所有图片格式使用相同的读取入口和缓存操作。
4. 减少 RAW、HEIF、TIFF 等文件中分散的缓存实现，而不是在旧实现外再包一层。
5. 解码或提取一次，产物同时供 UI 和缓存消费；UI 不必等待缓存持久化完成。
6. JPG 可以直接供 UI 的 `<img>` 显示，不要求 Rust 先解码成 RGBA。提取出的 JPEG 字节也可直接展示并原样缓存。
7. 保留 `oxy_media::preview(...)` 作为统一入口，保留 dispatcher。**不新增与 dispatcher 重复的 MediaProvider。**
8. 缓存是可重建基础设施，不成为与现有 library projection 竞争的业务状态权威。

“同时输出”指共享同一产物并独立消费，不保证两个消费者同时完成，也不承诺跨 WebView 零拷贝。

## 2. 当前代码与问题

- `crates/oxy-media/src/cache/key.rs`：源身份、策略和尺寸组合成独立 key，缺少同图产物集合。
- `crates/oxy-media/src/pipeline/artifact.rs`：按固定 512/4096 档位查询更大缓存，不能统一覆盖 Full。
- `crates/oxy-media/src/pipeline/raw.rs`：自行处理 embedded、developed、Full 的缓存查找和复用。
- `crates/oxy-media/src/pipeline/heif/artifact.rs`：自行处理缓存路径、embedded 快速路径、preview 和 Full 复用。
- `crates/oxy-media/src/media_source.rs`：混有缓存文件校验和损坏清理。
- `crates/oxy-media/src/cache/store.rs`：当前维护逻辑面向平铺文件，不能直接用于新分目录布局。
- `apps/desktop/src-tauri/src/jobs/preview.rs`：请求和 projection 按 level 分开，需要接通跨等级复用。
- `crates/oxy-media/src/presentation.rs`：已有表示与色彩契约，应保留并扩展，而不是仅按尺寸替代。

`RenderLevel` 是请求阶段，不是真实产物清晰度。例如 Sony HIF 160px embedded 可以作为 Preview 阶段返回。不能直接使用 `cached.level >= requested.level` 判断满足关系。

实施前重新检查工作区现状；本节是制定计划时的代码观察，不是永久有效的实现描述。

## 3. 目标架构

```text
调用方
  ↓
preview(...) / 统一读取协调层
  ├─ 解析格式、等级和显示策略（轻量，不触发完整解码）
  ├─ MediaCache.lookup(request)
  │    └─ 满足需求 → 发布资源句柄 → UI
  └─ 未命中
       ├─ 按现有格式/优先级规则取得执行资格
       ├─ 统一复查缓存
       └─ dispatcher → RAW / HEIF / system / 原文件路径
                          ↓
                      ProducedArtifact
                          ↓
                      统一产物发布层
                        ├─ UI：资源 URL / 现有 tile 通道
                        └─ Cache：有界后台持久化
```

### 职责边界

| 模块 | 职责 |
| --- | --- |
| 统一读取协调层 | 查缓存、复查、调用 dispatcher、统一发布结果 |
| dispatcher / policy | 需求解析、格式分发，保留现有后端选择与回退规则 |
| 格式 pipeline / backend | 提取 embedded、解码、显影、格式专属回退，报告产物事实 |
| 产物发布层 | 资源句柄、共享数据、UI/缓存分发、生命周期与背压 |
| MediaCache | 身份、匹配、索引、持久化、失效、维护 |
| 现有调度器 | 优先级、消费者订阅/取消、排队和 projection 发布 |
| Tauri | 薄协议/命令适配，不承载可复用缓存业务逻辑 |

协调公共流程不意味着把所有解码统一到一个锁。继续保留 RAW Full、HEIF 等原有资源限制，避免慢速 Full 阻塞便宜预览。

## 4. 请求、产物和满足关系

### 请求

```text
CacheRequest
  source_revision       源文件身份与版本
  requirement           显示尺寸 / 原生细节要求
  presentation_policy   可接受的表示、色彩、方向、锐化等策略
  allow_interim         是否允许低清过渡图
```

对外继续使用 Thumbnail / Preview / Full。内部尺寸由媒体策略决定，不让前端维护另一份尺寸或格式策略表。只在确有必要时探测源尺寸，不能为热缓存查询无条件执行昂贵 RAW/HEIF 探测。

### 产物事实

```text
MediaArtifact
  artifact_id
  source_revision
  actual_dimensions     方向归一化后的实际显示尺寸
  representation        Original / Embedded / Decoded / Developed
  presentation          色彩、方向、裁剪、锐化等契约
  policy_revision
  payload / location
  byte_size             已知时记录
```

按真实能力匹配，不按最初的请求等级定级。Thumbnail 请求如果提取出 7008px JPEG，后续可以按实际能力复用。

### 返回状态

- `Satisfied`：满足当前请求。
- `Interim`：可以展示，但不能记为高清需求完成。

Interim 是否触发后续升级由业务策略和调度器决定，不由缓存启动解码。第一版保留显式 Full 请求流程，不因为返回过渡图就无条件启动昂贵 Full。

### 统一匹配规则

实现独立、可测试的 `satisfies(artifact, request)`：

1. 源版本相同。
2. 策略版本和显示处理兼容。
3. 表示契约兼容；不能无条件混用相机预览与 RAW 显影。
4. 实际显示尺寸、细节能力满足要求。
5. 文件或内存资源有效。

Full 是细节/表示要求，不只是最大的枚举值。保留现有 RAW 相机预览覆盖源尺寸的显式替代策略。小原图达到原生细节即可满足，不通过放大伪造清晰度。

先查找 Satisfied，再考虑 Interim。在合格候选中优先选尺寸合适、读取成本低的产物，不默认选最大图。

| 已有产物 | 请求 | 选择 |
| --- | --- | --- |
| 512、4096、Full | Thumbnail | 优先 512 |
| 4096、Full | Thumbnail | 复用 4096 |
| 兼容 Full | Preview | 复用 Full |
| 160 embedded、满足需求的 decoded | Preview | 优先 decoded |
| 只有 160 embedded | 高清 Preview | 允许时返回 Interim |
| 800px 原图 | 4096 目标 | 按原生细节规则判断，不放大 |
| 大尺寸但色彩/表示不兼容 | Full | 不命中 |

## 5. 统一产物及 UI/缓存双输出

不强制所有后端输出 RGBA。示意类型：

```rust
enum MediaPayload {
    Encoded(Arc<EncodedImage>),
    Pixels(Arc<PixelBuffer>),
    File(Arc<ArtifactFile>),
}
```

| 产物形态 | UI | Cache |
| --- | --- | --- |
| 原 JPG/PNG/WebP 文件 | 受控 URL → `<img>` | 不强制复制；原文件不是可淘汰缓存 |
| 提取出的 JPEG 字节 | 媒体协议 → `<img>` | 原样落盘，不重新解码/编码 |
| 像素缓冲 | 媒体协议 + canvas/tile 渲染，或共享编码后的图像 | 后台编码、落盘 |
| 原生后端输出文件 | 受控 URL | 接管并原子提交，避免重复解码 |

JPEG 的解码由 WebView 负责。“统一读取”是统一获取可显示资源，而非所有格式必须经过 Rust 像素解码。

像素不能直接作为 `<img>` 的 JPEG 使用。如果 UI 与缓存都需要同一种编码结果，应共用一次编码；如果编码是 UI 的必要步骤，首次展示仍需等待该步骤，不能承诺完全无等待。

### 资源句柄

现有只返回最终缓存路径的结果需要扩展为资源描述，示意：

```text
MediaResource
  resource_id
  url
  dimensions
  representation
  satisfaction
```

可使用 `oxy-media://resource/<id>` 一类稳定 URL；实施时复用/扩展现有媒体协议，避免创建重复协议体系。句柄必须绑定源版本、产物和表示，不允许同一个 URL 静默变成另一张图片或不同视觉产物。

资源起初可由内存支撑，持久化后转为文件支撑；只对等价内容切换，并保证在途读取安全。如果落盘编码改变内容，应使用独立 variant/句柄，而不是在不可变 URL 下换内容。

IPC 只传句柄、路径和元数据，不通过 JSON 传图像字节。资源注册表只允许访问已注册资源，不把任意用户提供的路径暴露给协议。

### 生命周期与背压

- 解码器只产生数据，不直接调用 UI 或缓存。
- Rust 内用共享所有权避免重复复制大缓冲，不宣称 WebView 零拷贝。
- 缓存写入失败不撤销已成功展示的资源；记录诊断并释放失败任务资源。
- UI 取消只释放其订阅；缓存消费者仍使用的数据不能提前销毁。
- 编码与持久化使用有界队列、独立内存预算，不能积压多张全尺寸图耗尽内存。
- 超预算策略必须明确：背压、跳过可重建缓存写入或释放无人使用资源；不能静默丢弃 UI 所需数据。
- 临时文件由句柄持有；提交重命名须兼容 Windows 文件共享语义和在途读取。
- 原生文件型后端仍需先完成自身输出，双输出不保证该路径能逐字节提前展示。
- HEIF 现有渐进 tile session 保留；第一期不强制改造其传输为完整文件。其确实生成的完整、兼容产物才登记到文件缓存。

## 6. Cache 接口与分散实现收敛

缓存 trait 只抽象存储操作，不抽象格式分发。示意：

```rust
trait MediaCache: Send + Sync {
    fn lookup(&self, request: &CacheRequest)
        -> Result<Option<CacheHit>, MediaError>;

    fn publish(&self, artifact: PendingArtifact)
        -> Result<MediaArtifact, MediaError>;
}
```

提供 `DiskMediaCache` 和测试替身。同步接口在 blocking worker 中执行，UI 发布不等待后台 `publish` 完成。维护接口可留在具体服务，不把所有能力塞进读取 trait。

需要迁移/删除的分散代码：

- RAW：`cached_embedded`、`cached_developed`、`cached_result`、`larger_cached_preview`。
- 公共 pipeline：`larger_cached_decoded_preview`。
- HEIF/system：自行拼 key/路径、检查文件、查找 Full 或较大尺寸。
- `media_source.rs`：`cached_preview_result` 中的缓存有效性和损坏清理。
- 格式代码内的缓存临时文件管理、原子提交、登记和淘汰。

后端如必须写文件，由公共层提供暂存目标，后端仅写产物。所有权、发布、清理由公共层负责。编码器本身可复用，不要求把 JPEG 编码算法硬塞进 store。

**验收标准：格式 pipeline 不再知道缓存命名、布局或跨等级查询规则。仅允许受控的产物输出目标，不保留各格式自己的 lookup/publish 流程副本。**

## 7. 持久化设计

### 身份

```text
SourceRevision  = canonical path + 文件身份 + 大小 + 高精度修改时间
VariantIdentity = 表示契约 + 处理策略版本 + 目标规格
```

实际尺寸另存，不能由目标规格推断。第一期沿用现有源指纹原则，不读整文件计算 hash，避免大文件/NAS 代价；明确其不是内容级校验。保留 symlink 别名归一语义，不额外承诺 hardlink 去重。

### 布局

```text
media-cache-v2/
  <source-prefix>/
    <source-revision>/
      manifest.json
      <artifact-id>.jpg
      <artifact-id>.jpg
```

- manifest 记录同源版本产物集合。
- 按图片查询，不扫描整个缓存目录。
- 有容量上限的内存 manifest 索引，重启按需加载。
- 同一实际产物可满足多个等级，不按请求等级重复保存。
- 不新增与 library projection 重复的业务状态数据库。

### 原子提交与恢复

1. 生成暂存产物并校验尺寸、表示。
2. 原子提交不可变文件。
3. 同源写入协调下合并并原子更新 manifest。
4. 更新内存索引/资源位置。

并发写入必须合并集合，不能覆盖其他 variant。明确进程间协调：若允许多进程共用目录则增加文件级协调；不能仅依赖进程内 mutex 宣称安全。

崩溃孤儿由后台清理。manifest 指向缺失/损坏文件时按 miss 修复；权限等错误不能全部伪装成 miss。原文件引用显式区别于 `ManagedArtifact`，永不由缓存删除。

## 8. 调度、失效与维护

第一期保留现有任务调度和格式专属解码限制，统一执行资格获取后的缓存复查。先解决缓存正确性，不同时重写所有任务合并逻辑。

后续跨等级合并：

- 低等级可订阅正在生成的兼容高清任务。
- 便宜 embedded 路径不被慢速 Full 强制阻塞。
- 单消费者取消不影响其余消费者。
- 保留优先级升降，不承诺抢占运行中的原生解码。

源版本变化后，旧资源不能发布到当前请求或 projection。存储前后复核与现有 generation/过期结果拒绝配合；缓存索引和业务 projection 不互相替代。

维护要求：

- 第一版可保留现有淘汰顺序，LRU 优化另行度量。
- 保护写入中、UI 正在使用的资源，采用引用/有界租约或现有会话释放信号，漏释放不能永久占用磁盘。
- 更新 manifest、内存索引和资源注册表的一致性。
- 只遍历缓存拥有的固定目录结构；不跟随 symlink 越界，不递归删除任意配置路径。
- clear 使用 generation 隔离旧在途写入，避免清空后被旧任务重新填充。
- 明确清理时活跃 UI 的处理：可延迟删除已租用资源，但失效后不得作为新请求命中。
- v2 使用版本隔离，旧缓存惰性淘汰；启动/打开文件夹不全量迁移。

## 9. 格式迁移与应用边界

迁移顺序建议 Raster/TIFF → RAW → HEIF：

- Raster：保留原文件直接显示，不强制建立副本或 RGBA 缓存。
- TIFF：保持系统后端，记录实际尺寸，迁走存储逻辑。
- RAW：保留 embedded 优先、显影 fallback 和 Full 替代策略；JPEG 字节双输出。
- HEIF：优先查询真正满足请求的缓存，再提取过渡图；保留平台后端和 tile session。

跨边界修改涉及：

- `crates/oxy-domain`：公共序列化资源契约，camelCase。
- `apps/desktop/src-tauri/src/jobs/preview.rs`：跨等级查询、资源就绪与持久化状态分离。
- Tauri 媒体协议及 session 适配：受控资源访问和释放。
- `apps/desktop/src/lib/api.ts`：类型、IPC 包装、浏览器 demo。
- 前端图片/渲染消费处：接受资源 URL，正确处理 Interim、切图和资源生命周期。

projection 必须：

- 不把 Interim 永久视为高清完成。
- 不让旧低清 projection 长期绕过更好的缓存候选。
- 区分可展示与已持久化；内存句柄不能作为跨重启有效路径持久化。
- 文件失效后能重新查找或生成。
- 保留 revision/generation 和迟到结果拒绝。
- 使用媒体层匹配规则，不另写前端等级比较表。

## 10. 分阶段交付清单

### 阶段 1：契约和基线

- [x] 重新盘点当前调用者、协议、锁及缓存路径。
- [x] 定义请求、实际产物、Satisfied/Interim 和表示兼容规则。
- [x] 建立纯函数匹配测试及现有冷/热性能基线。
- [x] 确定资源生命周期、跨进程缓存目录所有权与背压策略。

### 阶段 2：多等级缓存核心

- [x] 实现源/variant 身份、manifest、bounded 内存索引。
- [x] 实现统一 lookup、原子提交、损坏恢复和并发合并。
- [x] 实现 v2 维护、失效、清理 generation 和活跃资源保护。
- [x] 建立测试用缓存实现。

### 阶段 3：发布层与 JPEG 端到端路径

- [x] 实现共享产物、稳定资源句柄和受控协议。
- [x] 先打通原 JPG 直接显示、embedded JPEG 同时供 UI/缓存。
- [x] 打通后台写失败不影响展示、取消与内存回收。
- [x] 保留旧接口兼容适配，逐步接入调用方。

### 阶段 4：格式与应用迁移

- [x] 迁移 Raster/TIFF、RAW、HEIF，删除格式内缓存查询/命名/提交实现。
- [x] 对像素和原生文件产物接入公共发布流程。
- [x] 更新 domain、Tauri、API、demo 和前端消费。
- [x] 接通 projection 跨等级命中、Interim 和持久化状态。
- [x] 保留渐进 tile 行为与平台回退。

### 阶段 5：验收和按需增强

- [x] 完成本机正确性、并发、重启/损坏恢复和清理竞态验证。
- [x] 使用隔离 app-data/cache 的 packaged macOS WebView 跑 JPEG/HIF/RAW 冷热样本和 100k 目录样本，并诚实记录未达标/缺失 mark。
- [x] 实施 capability-aware 跨等级在途产物共享，包含 persistence 尚未完成的窗口；Interim 的高等级请求继续升级。
- [x] 实测未证明需要首次读取生成全套后台缩图，因此保持按需生成，不增加该成本。
- [x] 更新正式架构、media README 与性能文档。
- [x] 记录 100k 隔离冷启动样本，不将 300 ms 作为本任务严格完成门禁（用户后续决定）。最新三次 median 197 ms、P95/最大值 637 ms；不宣称尾延迟达标。
- [ ] 在 Windows/Linux runner 验证文件锁、原子替换、sharing/rename 和 packaged WebView 协议 URL；这不是可由本机替代的平台门禁。

## 11. 验证标准

### 正确性与资源安全

- 同图多清晰度共存、向下满足、不能向上冒充。
- 实际大 embedded 可复用；小原图、方向、色彩、锐化、RAW 表示规则正确。
- JPEG 字节展示/落盘不重复解码编码；双消费者不重复执行源解码。
- UI 就绪不依赖缓存写完，缓存失败不撤销展示。
- 切图、源修改、取消、迟到结果不串图。
- 损坏/缺失、重启、并发发布、clear 与写入竞争、崩溃孤儿恢复。
- 有界内存和队列，资源释放、租约超时、Windows 在途文件读取安全。
- 原文件不被删除，协议不暴露任意路径。
- 格式文件中不再残留独立缓存命名、查找和跨等级复用实现。

### 性能

遵循 [PERFORMANCE.md](../PERFORMANCE.md) 与 [PERF_E2E.md](../PERF_E2E.md)：

- 100k 文件目录首屏 300ms 保留为参考目标，不作为本任务严格完成门禁（用户后续决定）；仍不得增加媒体扫描或同步索引。
- 热缓存 loupe 预览 <150ms。
- 冷选中图片预览 <800ms；Full 独立测量，过渡图保持可见。
- 测量 WebView 首次显示、编码、持久化、协议读取、峰值内存、滚动主线程耗时。
- 高清替代缩略图可能增加浏览器解码成本，需要实测，不把“避免源解码”当成端到端必然提速。
- 记录基线和前后差值；已不达标路径不得因重构结束就标记达标。

跨边界实施后运行：

```bash
pnpm check
pnpm test
pnpm build
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Rust 实施遵循 [RUST_STYLE.md](../RUST_STYLE.md)。真实 fixture 缺失导致跳过的检查必须明确报告，不当作验证通过。测试结束关闭自行启动的 debug 实例。

## 12. 非目标与后续设计约束

- 不引入第二套 dispatcher/MediaProvider。
- 不强制所有图片解码成 RGBA，也不强制复制原 JPG。
- 不第一期统一 HEIF tile session 与完整文件缓存的全部生命周期。
- 不重写现有 decoder 算法、平台选择或回退顺序。
- 不承诺跨 WebView 零拷贝、原生解码抢占或文件输出后端的提前流式展示。
- 不一次性重写调度、缓存、解码全部机制。

实施采用既有 `oxy-media` 协议的受控 resource/tile URL、显式 renew/release、64 项/128 MiB
resource registry、有界 publisher 队列与跨进程 manifest/file locks。Windows 使用同一前端 helper
把 custom scheme 规范化为 WebView2 HTTP 形式；其真实 packaged 行为仍需 Windows runner 验证。

## 相关文档

- [媒体架构](../architecture/03-preview-pipeline.md)
- [HEIF tile session](../architecture/04-heif-tile-session.md)
- [数据与状态](../architecture/05-data-and-state.md)
- [已有媒体模块整理](../architecture/07-media-refactoring.md)
- [已完成的 lib.rs 简化任务](oxy-media-lib-refactor.md)

本计划是行为重设计，不是上述已完成文件拆分任务的等价代码移动。实施后再将已验证结果同步到正式架构文档，不能提前写成已落地。

## 实施记录：阶段 1–3 核心基础（2026-09-07）

本次只落地媒体 crate 的新核心和 Tauri 受控协议分支，保留旧 dispatcher、旧平铺 cache API 与
当前应用结果契约。格式 pipeline 和应用尚未迁移，所以阶段 3 的“原 JPG / embedded JPEG 端到端”
仍未勾选，不能把本记录视为阶段 4 或整个目标完成。

### 已盘点边界与实施选择

- 当前生成调用仍由 `oxy_media::preview` dispatcher 进入 RAW、HEIF、system executor；旧 executor
  仍自行使用平铺 key/path。`CacheManager` 仍调用旧的 usage/prune/clear 兼容 API，避免本步扩大到
  应用迁移。HEIF tile 继续使用既有 `oxy-media://.../tile/...`、decode gate、RAW Full 独立 lane、
  HEIF cache-write lane 和同源锁。
- v2 位于旧 preview 父目录下独立的 `media-cache-v2/`。源 revision 使用 canonical path、平台文件
  identity、大小和高精度 mtime；artifact ID 还包含 variant、实际尺寸和编码字节 hash。原文件只进入
  资源注册表，绝不写入 managed manifest 或被 cache maintenance 删除。
- 匹配使用 `CacheRequest`、真实尺寸、native-detail 事实、表示、方向、色彩、锐化和 policy revision。
  先选 `Satisfied`，再选显式允许的 `Interim`；同状态优先较小的合格产物。RAW camera JPEG 满足 Full
  仍要求调用方显式允许且 display-space 双边达到 90%，不会按枚举等级或单纯大尺寸冒充显影。
- manifest 先提交已同步的不可变 artifact，再用同目录临时文件原子替换并同步目录。进程内操作锁加
  `fs2` 文件锁协调并发进程；全局 shared/exclusive lock 隔离 publish/lookup 与 clear，source lock 合并
  同源 variants。损坏/缺失 artifact 按 miss 修复 manifest，权限和其他 I/O 错误继续上报。
- generation 持久化在 v2 根目录；旧 generation 的后台 publish 会被拒绝。命中产生默认五分钟的有界
  lease；跨进程 lease marker 让 prune/clear 延迟删除活跃 artifact，超时 marker 可回收，避免崩溃后
  永久占用。manifest 内存索引容量由构造参数明确限制并按 LRU 淘汰。
- 资源 URL 为不可变 `oxy-media://localhost/resource/<Rust-generated-id>`。协议只能解析注册表中的 ID；
  不能把 URL path 当文件路径。Tauri 当前给注册表 64 项/128 MiB 内存上限（低于现有浏览器
  512 MiB 上限）。发布器使用可配置有界 `sync_channel` 和独立 pending-byte 上限；任一预算满时跳过
  可重建持久化，不撤销 UI 资源。失败诊断只保留最近 128 项。
  encoded JPEG 的 UI 和 cache job 共享同一个 `Arc<[u8]>`；UI handle 释放不提前销毁 cache consumer。
- 测试替身为执行同一匹配函数和 generation 规则的 `MemoryMediaCache`，没有新增 dispatcher 或
  `MediaProvider`，也没有修改 decoder、vendor、fallback 顺序或通过 JSON 传图片字节。

### 基线与尚未完成的性能验证

变更前本机 `cargo test -p oxy-media --lib` 列出 89 项测试，实际为 80 passed / 9 ignored，约
6.54 s（`time` wall 6.69 s）。现有冷/热交互数值继续以 `PERFORMANCE.md` 的同机 release/E2E 记录为
基线；本步没有可比较的打包 WebView E2E fixture sweep，不能宣称 150 ms/800 ms 预算或跨平台性能
已验收。新增 lookup 仍会校验候选图片头，跨进程 lease/maintenance 会扫描 v2 固定两层目录；这些
成本需要在阶段 4 接入真实 pipeline 后单独测量。

### 后续接缝

- 把 Raster/TIFF → RAW → HEIF executor 迁到 `DiskMediaCache`/`ArtifactPublisher`，删除格式内旧
  key、lookup、命名和 commit；每个格式必须正确构造 presentation/native-detail 事实。
- 从 preview queue/projection 发布资源 descriptor、Interim 与 persistence 状态，增加显式资源释放；
  当前前端仍消费旧文件 path，尚未注册原 JPG 或 embedded JPEG 到新协议。
- 让 `CacheManager` 的设置、usage/prune/clear 管理 v2（并继续惰性处理旧平铺缓存），随后补 domain、
  TypeScript/demo 和组件契约。
- 在 macOS/Windows/Linux 验证 manifest 原子替换、文件锁和在途读取语义；用真实 RAW/HEIF/TIFF
  fixtures 重跑冷/热 E2E、协议读取、WebView 首次显示、峰值 RSS 和大图向下复用成本。

## 实施记录：格式 cache 迁移里程碑（2026-09-08）

本里程碑把 TIFF/system、RAW 和 HEIF 文件 artifact 的生产 cache 路径迁入 `ArtifactCache` +
`DiskMediaCache`。Raster 原文件继续走 dispatcher 的直接文件路径且不复制。格式 executor 不再拼
cache key、artifact 文件名或执行 cache 原子提交；需要文件输出的原生 backend 只获得公共层创建的
受控临时目标，完成后公共层按真实尺寸和表示事实发布。旧 key 实现和 `media_source` flat lookup 已
删除；旧 flat store 只作为当前 CacheManager 的惰性清理兼容边界保留。

- RAW thumbnail/preview 统一按真实长边向下复用 embedded/developed/full artifact；Full 仍使用独立
  decode lane，并且只有显式 RAW camera-preview 策略加双边 90% 覆盖才替代 developed native detail。
  camera JPEG 记录 metadata orientation 与 embedded/unknown color，developed JPEG 记录 applied
  orientation、sRGB ICC 和 native-detail 事实。
- HEIF 统一查询 decoded/native 与 Sony embedded 表示。先找 Satisfied，再只允许 embedded 作为
  Interim，避免 512 decoded 永久满足 4096；Full cache 查询不再为热命中探测源尺寸。既有 backend
  plan、decode gate、取消检查、Full fallback 和 tile session 保持。
- TIFF system 输出记录 applied orientation、embedded/unknown color 和 System 表示。原生 backend
  仍按原平台能力工作，不改变 unsupported 平台行为。
- manifest 校验允许 metadata orientation 的编码宽高与显示宽高交换。并发 publish 在 source lock
  下强制读取最新磁盘 manifest，不以可能具有粗粒度时间戳的内存索引做写决策。
- `CacheManager` 的 settings/size/prune/clear 同时覆盖 v2 与旧 flat cache；v2 优先占用总预算，旧缓存
  使用剩余预算并惰性淘汰。当前返回 artifact 在两种布局中都可作为本轮 prune 的显式保护路径。

仍未完成并保持未勾选：生产 `preview(...)` 目前仍为兼容现有 projection 同步返回 managed file path，
尚未把 `ResourceDescriptor`、Satisfied/Interim、persistence 状态传入 domain/TypeScript/UI；因此新的
`ArtifactPublisher` 共享内存 JPEG 与后台持久化能力尚未成为正常 app 请求的可观察路径，不能声称 UI
已独立于持久化。RAW camera JPEG 的 EXIF orientation 对实际 display dimensions 的精确归一仍需在
接入资源 DTO 时补 fixture 验证。HEIF tile 的完整产物继续登记 v2，但 tile 传输生命周期没有重写。
本里程碑未运行会清应用缓存的 E2E runner，也未更新性能 baseline。

## 实施记录：移除旧缓存兼容（2026-09-09）

应用缓存管理现只使用 `DiskMediaCache`：settings/usage/prune/clear 不再调用旧 flat store API，
`oxy-media` 也不再导出该兼容层。启动时会清理当前 cache 位置及已知默认 preview 目录第一层的
pre-v2 普通文件；切换到自定义位置时同样先执行一次该清理。清理不递归、不跟随链接，
`media-cache-v2` 和其他子目录保持不变。运行期 recency、容量和在用保护统一由 v2 manifest、
generation 与 lease 管理。

## 实施记录：应用发布接入里程碑（2026-09-08）

生产 `PreviewQueue` 现调用保留 dispatcher 的 `preview_for_app`：磁盘命中和原始 JPEG/PNG/WebP
注册为受控文件资源；新生成的 TIFF、RAW 与 HEIF JPEG 先把共享编码字节注册为资源，再进入有界后台
持久化队列。旧 `preview` 同步 managed-path API 保留给兼容调用和测试。domain、TypeScript 和
projection 已传递 resource descriptor、Satisfied/Interim 与 Pending/Skipped/Persisted 状态；前端优先
使用资源 URL。原始浏览器可解码格式只请求一次 thumbnail 语义资源并跨 loupe 等级复用，不复制原文件。

资源注册表使用两分钟可续租的 UI lease 和独立协议读取 lease；组件挂载/切图时续租或释放，未被前端
接收的取消结果最终超时可淘汰。64 项/128 MiB 注册表及发布队列/bytes 上限继续提供背压；后台写失败或
跳过不会撤销已发布资源。projection 仅在 managed path 已存在或本进程资源 ID 仍存活时有效，因此瞬态
URL 不会在重启后冒充持久 cache。浏览器 demo 对新增可选字段保持兼容，资源续租/释放为无操作。

RAW embedded JPEG 尺寸现在从 JPEG header 与 EXIF orientation 归一，不再为读取尺寸执行完整像素解码；
已增加方向交换单元测试，但仍缺外部相机 RAW fixture 的端到端方向验证。HEIF tile session、backend
顺序、decode gate、同源锁、RAW Full lane 与取消语义未改写。工作区 Rust 测试、Clippy、前端类型检查、
单测和 production build 已通过；尚未执行隔离 app-data 的 packaged WebView E2E、性能基线或跨平台
原子替换/锁/协议在途读取验证，因此阶段 5 保持未勾选。

## 实施记录：应用生命周期、在途共享与本机验收（2026-09-08）

本里程碑关闭应用层 review blocker。SQLite 只序列化可跨重启的 artifact 事实，剥离当前进程的
resource descriptor；恢复时若文件仍有效，应用重新登记带新进程 namespace 的受控 resource，managed
artifact 同时恢复磁盘 lease。resource ID 使用进程 nonce 和进程全局计数，不会在独立 registry 或重启
后从 `resource-1` 复用。CSP 的 `img-src` 同时允许 macOS/Linux custom-scheme 形式和 Windows WebView2
的 `http://oxy-media.localhost` 形式；全局 asset protocol 和 `**/*` scope 已移除，原始 JPEG/PNG/WebP
的显示和预加载也走 registry。所有 WebView raster preload（direct/generated/filmstrip）统一通过有优先级的
串行 presentation queue，不再绕过并发控制。

Interim projection 仍可保留显示，但不再成为 Preview/Full 的终态。queue 在发出 Interim 后继续运行一个
不允许 Interim 的升级请求，Satisfied 结果按原 generation/revision fence 替换它。publisher 保存按真实
能力匹配的进程内在途产物；跨 level 等待者可在落盘完成前订阅同一 encoded/staged resource，不重复源
解码。RAW full 仍使用独立 lane，Sony embedded 探测仍在慢 HEIF decode lock 之前，未改变 backend 顺序。

encoded 与原生 staged-file 都先注册 UI resource，再异步持久化。staged 文件先原子移动到 publisher
持有且位于 v2 cache tree 之外的进程临时目录，cache clear 不会删除这个 UI 文件；cache worker 以文件系统
copy 取得自己的提交输入（不重复 decode，也不读入第二个大内存 buffer），原子提交后
仅在协议 read lease 释放后把同一 resource ID 切到 managed file 并删除 staging。成功完成会把仍是当前
revision 的 projection 从 Pending 更新为 Persisted，并在写入完成时执行容量清理；失败改为 Skipped 而不
撤销已显示资源。v2 文件保持不可变，应用不再用 `set_times` 更新它们，否则会使已记录 file revision 的
resource 失效。

协议 `materialize` 的 4-response/128 MiB reservation 只覆盖 Rust 读取/复制到 `Vec<u8>` 的阶段。Tauri
`Response<Vec<u8>>` 接管 body 后 reservation 已释放，当前 API 没有 response-drop callback，因此不能声称
限制覆盖 WebView 消费完毕前的 Tauri-owned body lifetime；这是准确的现有平台 API 边界。

本机 macOS release packaged WebView 单样本（runner 的 data/cache 均在
`tests/perf/.reports/.runtime/`，没有读取或清除正常用户 cache）：cold/warm JPEG first preview 61/57 ms；
cold HIF first preview 62 ms、first tile 461 ms；cold/warm ARW first preview 62/39 ms。warm HIF 记录
first preview 38 ms、first/all tile 336/491 ms，但场景等待 `image:loaded@full` 超时，不能把 full 预算
标为通过。100k 合成目录 first-page paint 为 1039 ms，仍未达到 300 ms 预算。以上均为一次运行，不更新
baseline，也不代表 P95 或 Windows/Linux。真实 `DSC02948.ARW` embedded preservation/orientation 测试通过；
`DSC00529.ARW` 的旧“必须缩到 512”fixture 断言已按保留真实 embedded 能力的契约修正后通过。

## 实施记录：最终应用 review 与验收修复（2026-09-08）

最终 review 中的具体实现问题已关闭：

- 重启恢复 managed projection 不再只看 `is_file`；`DiskMediaCache::validate_and_lease_path`
  在 cache/source locks 下核对固定布局、当前 generation、manifest source revision、当前源文件身份、
  artifact 长度/尺寸和 JPEG EOI，并修复无效 manifest entry。原文件仍按 registry 的 observed file
  revision 注册。应用 projection identity 现在包含与媒体 cache 相同的 canonical path、平台 file
  identity、大小和纳秒级 mtime 派生 revision ID。
- resource 与 tile 都通过同一个 `mediaProtocolUrl` helper；Windows WebView2 得到
  `http://oxy-media.localhost/...`，macOS/Linux 保留 `oxy-media://localhost/...`。已有 TypeScript
  测试覆盖两种形式；Windows packaged WebView 仍是平台门禁，不以 user-agent 单测冒充实机通过。
- Interim 首次可显示回复后，`PreviewQueue` 保留 active request、waiter scope、priority 和 cancellation
  token，再执行禁止 Interim 的升级。前端在 Satisfied/error/source replacement 或超时前保留 abort
  subscription；最后一个 consumer 离开会取消 backend。兼容 decoder-running work 继续由媒体层
  `coordinate_work` 单飞，RAW Preview/Full development 不重复执行。
- completion-time prune 使用 pending latch；重叠 publication 会让持有 worker 再跑一轮。成功 publication
  的 active metadata 在 transition 后删除，失败/stale metadata 依 registry 全局回收，均有有界测试。
- HEIF tile session 现在把已经取得的未锐化、orientation-applied sRGB 像素直接编码为 native Full
  artifact，不再调用 full pipeline 重新解码源文件。Windows FFmpeg 路径从已经取得的 JPEG tiles
  拼接同一规范 buffer；显示锐化只在 tile crop 时读取完整图邻域，不污染 cache identity 或产生 tile
  边界接缝。macOS 使用 ImageIO 从该 buffer 写带 sRGB 色彩空间的 JPEG，session source decode 仍为一次。
- warm HIF harness 明确命名为 `warm-loupe-hif-tiles`，等待 tile 完成；warmup 必须 complete，留出
  persistence settle，并验证 managed artifact。测量 run 若不是 `cachedArtifact` backend 会被拒绝，
  不再把失败 warmup 或 source decode 标成 warm cache。

最终 macOS release packaged-WebView 证据使用 runner-owned data/cache，JPEG/HIF/ARW 每条 cold/warm
route 各三次。first-preview median/P95：JPEG cold 86/89 ms、warm 70/72 ms；HIF cold 59/69 ms；
ARW cold 61/69 ms、warm 44/51 ms。`warm-loupe-hif-tiles` 三次均验证 `cachedArtifact`，first preview
45/53 ms、first tile 244/252 ms、all tiles 564/572 ms；这里只判定 150 ms warm preview 预算，未把 tile
完成伪装成 150 ms Full artifact 通过。

100k 场景改为每次都清理隔离 data/cache，避免把首个冷扫描和后续 warm SQLite 混成 median。三次
first-page paint 为 753/759/763 ms（median 759、P95 763），其中 first-page IPC return 为
645/646/659 ms，React commit 到双 rAF paint 为 104/108/113 ms。它比旧 1039 ms 单样本更快，但没有
同构建前后配对，不能据此声称重构提升或回归；300 ms 预算明确保持未通过。

## 实施记录：100k 首屏关键路径优化（2026-09-08）

本次先把扫描 progress 从每个目录项一次改为每 256 项一次，同时保留初始、枚举完成/属性阶段切换和
最终报告；完整非递归发现、sidecar 配对及每个文件的真实 size/mtime 均不变。E2E 的
`assets:first-page-returned` mark 现在携带 cache、resolve、enumeration、attributes、snapshot serialize、
snapshot persist、sort、native 与 IPC 分段，runner 记录这些指标但没有降低或替换 first-page paint
预算。名称排序为每个摘要只生成一次不区分大小写的 key，保留原比较器的大小写 tie-break、升降序和
跨页顺序；实测 sort median 从 95 ms 降至 10 ms。

冷扫描完成后先以 epoch/revision fence 原子发布完整内存 snapshot，再由容量 64 的单 worker 持久化。
每个目录至多排队一个 slot，较新 revision 覆盖 pending payload；worker 在序列化后重新核对 epoch、
revision 和 `Arc` 身份，失效任务不能复活 tombstone。队列满时发送端提供背压而不丢 durability；Library
析构会排队 shutdown 并 join，保证已接受的最新 snapshot 在正常重启前落盘。SQLite/JSON 持久化失败
只记录为可重建 cache failure，不撤销已返回的完整内存页；独立 persistence fence 避免 SQLite 写锁住
snapshot state，因此后续内存分页不等待后台落盘。测试覆盖 SQLite 被阻塞时的快速内存读取、persistence
failure 后的内存读取、失效 epoch 不能覆盖 tombstone、coalesced 最新 revision 的离线重启恢复，以及原有
分页 revision fence。

同一 runner、同一 100k fixture 和隔离 data/cache 下，持久化仍同步的三次实施中间样本 first-page paint
为 1121/698/703 ms（median 703；其中一次 enumeration 500 ms outlier）；median serialize/persist/sort
分别为 19/82/95 ms。最终 release 三次为 519/498/465 ms（median 498、P95 519），first-page return
为 462/419/446 ms（median 446、P95 462），return-to-double-rAF paint 为 57/79/19 ms（median 57、
P95 79）。最终分段 median/P95 为 enumeration 84/110 ms、attributes 330/341 ms、response-path
snapshot serialize 0/0 ms、snapshot persist/enqueue 0/0 ms、sort 10/11 ms、native 435/452 ms、IPC
437/454 ms。中间样本只是同次实施中的配对诊断，不作为历史 baseline 改善声明；正式 baseline 仍为空。
300 ms gate 继续失败且未降级，当前主要剩余时间是完整准确属性读取（median 330 ms）及 57 ms paint；
在不省略 size/mtime 或返回不完整 total 的约束下仍需后续 filesystem bulk-attribute 方案与实机证据。

本机运行的 frontend typecheck、137 项 Vitest、production build、Rust fmt、workspace Clippy 和
workspace tests 均通过；fixture/manual tests 按声明保持 ignored。release app 由 runner 每次结束时
强制关闭，未留下 debug 实例。Windows/Linux 的 file lock、atomic replace、sharing/rename、CSP/
protocol URL 与 packaged WebView 矩阵在本机不可获得，仍未验证，也没有发布、提交或推送。


## 最终本机接管与验收记录（2026-09-08）

用户要求取消全部 subagent 后，父级直接完成后续工作，没有再次委派、提交或推送。

- 确认并验证 HIF session 不依赖 RAF 启动；补充 StrictMode/暂停 RAF、artifact 租约、迟到响应和
  迟到 listener 释放测试。当前打包冷 HIF、warm cachedArtifact tile 各三轮通过。
- 修复 persisted projection 恢复时缓存命中诊断遗漏：重新验证并注册 managed 文件后报告
  `cached artifact`，不重放旧 decode timing；增加真实 manifest/restore 回归测试。
- 显式运行 RAW/HEIF ignored fixture 测试发现 ImageIO Full JPEG 省略 ICC。统一发布层仅对已被
  证明转换到 sRGB 的 native JPEG 补 APP2，不重解码/重编码、不重新读源图。64 KiB 分块倒移
  保留编码数据，测试覆盖跨块数据完整性和幂等性；真实 HIF Full ICC 测试现已通过。
- macOS `getattrlistbulk` 批量读取属性，portable fallback 保留。测试与 portable scanner 对比
  精确 size/mtime、大小写 XMP、symlink、broken link、目录不递归及非法名字引用；不对 benchmark
  fixture 做特例。首屏三次 median 197 ms、最大637 ms，不宣称尾延迟达标。用户后续决定取消
  300 ms 的严格任务门禁，停止继续扩大性能优化范围。
- 父级直接运行完整 Rust fmt/clippy/workspace tests、前端 check/test/build；媒体并发套件连续
  三次通过，未复现先前测试夹具锁死。最终完整 workspace suite 的媒体结果为 129 passed/9 ignored，
  Tauri 为22 passed/1 ignored；前端30个文件/144测试通过。完整命令输出保留于
  `/tmp/oxy-final-verified-rust.log`，打包采样输出为 `/tmp/oxy-media-final-e2e.log`（本机临时日志）。
- 真实 fixture：DSC00529.ARW extraction/Full、DSC02948.ARW embedded preservation、
  DSC00449.HIF Full 与 ImageIO fallback 显式运行通过，不把其余 ignored tests 算作执行通过。
- 最终 release 六个媒体场景各三轮通过：JPEG cold/warm 57/53 ms、RAW cold/warm 53/41 ms、
  HIF cold/warm 45/39 ms（first-preview median）。RAW warm 必须有持久化及 cache-hit 证据；
  HIF warm 必须为 cachedArtifact。完整采样和边界见 PERFORMANCE.md 的最新记录。

仍未完成的外部门禁：Windows/Linux 文件锁、原子替换、sharing/rename、原生 WebView 协议与
渲染矩阵。用户明确选择暂不提供环境，且未授权提交测试分支/远程 CI；本机为 Darwin，Docker
客户端对应 daemon socket 不存在。再次检查 Rust sysroot 也只有 `aarch64-apple-darwin`，没有
可用于本任务的 Windows/Linux 原生运行器。因此不能把全部跨平台目标标记完成。真实跨平台
峰值内存与滚动端到端资格也不由本机单图样本代替。

恢复验收所需外部动作：提供 Windows/Linux runner，或明确授权独立测试分支与 CI（不等于
合并/发布）。在对应平台针对本次完整工作区运行 workspace fmt/clippy/tests、前端 checks/tests/
build、Tauri packaged build，并使用隔离数据目录验证 resource URL/CSP、rename/lease/clear
竞态及媒体冷/热场景。记录平台、代码版本、原生测试日志和实际渲染结果；仅交叉编译或修改
浏览器 UA 不能替代这些门禁。当前没有为等待环境而启动常驻进程，也没有计划远程推送。

## 后续代码质量审查（保留端到端行为）

- 回归测试先复现 HEIF 两个迟到回调问题：`createImageBitmap` 完成后切图仍绘制旧 tile；旧
  session 启动失败仍更新新图状态。现已在 await 后检查生命周期、用 finally 释放 bitmap、
  清理失败监听器，并在切图/卸载时中止 fetch、清空队列；不改变解码顺序或 tile 并发上限。
- 缩略图/大图共享 resource ID 时，原先任一组件退出都会释放整个后端租约。现在由轻量本地
  引用计数协调最后使用者释放，并覆盖重复 cleanup、StrictMode 重挂载、迟到结果及 IPC 失败。
- 删除 cache/encode.rs 仅测试编译的旧 timed/byte/atomic writer 和重复 ICC 插入实现。取消、
  不覆盖已有 artifact、提交失败及临时文件清理测试直接调用生产 JPEG writer；重叠测试合并，
  明确检查 `.tmp` 为空，不再仅检查父目录文件数。
- 完整 Rust fmt/clippy/workspace tests 通过；前端31文件/149测试与check/build通过。重新
  打包，JPEG/RAW/HIF 冷热六场景各三轮全部通过。first-preview median 分别为58/49、51/41、
  51/39 ms；warm HIF all-tiles median544 ms。临时日志：`/tmp/oxy-quality-rust.log`、
  `/tmp/oxy-quality-build.log`、`/tmp/oxy-quality-e2e.log`。
- 本轮没有进行大规模模块拆分、改变缓存策略或扩大性能优化范围；跨平台验收状态保持不变。

## 资源预算与生命周期修复计划（已实施，2026-09-09）

### 问题与已确认结论

实际浏览 Sony HIF 时出现：

```text
full-detail HEIF decode failed: media resource budget is exhausted
failed to publish HEIF full projection: media resource budget is exhausted
```

这里的 resource 是 `oxy-media://localhost/resource/<id>` 对应的后端注册记录，包含不可变资源 ID、
尺寸、MIME/表示、文件版本、原文件/managed artifact/staged file 路径或共享 encoded bytes，以及 UI、
协议读取和缓存 artifact lease。它不是解码线程或 GPU 纹理；文件型资源通常不长期打开文件句柄，且在
registry 中记为 0 encoded bytes，但仍占一个 entry，并可能保护 staged 文件或 managed artifact。

当前所有资源统一受 64 项/128 MiB 限制。每张图片的 thumbnail、preview、full 都可能各占一项；
`Thumbnail` 当前还会同时续租三个 projection，而不是只保留正在显示和准备替换的资源。新资源注册时
自动获得 120 秒 UI lease，前端每 60 秒续租；当 64 项都处于 UI/read lease 时，LRU 无法淘汰第 65 项。
HEIF Full 使用 staged/managed JPEG，encoded 内存占用为 0，因此本次首个失败确定属于 entry 容量，
不是 128 MiB encoded 内存耗尽。dispatcher 又把发布容量错误当成可回退的 HEIF 解码错误，重复尝试
8192px projection，产生第二条同因错误和不必要工作。

资源预算必须继续有界，以防漏 release、encoded bytes、staged 文件、artifact 保护和 custom protocol
URL 无限增长；但 entry 数量、encoded 常驻内存和协议临时响应不能继续共用同一个过低预算。

### 目标行为

1. 正在显示的图片及正在完成浏览器加载的替代图不被淘汰。
2. full 真正显示后立即释放已被替代的 thumbnail/preview；稳定状态每个组件通常只持有一个资源，渐进
   切换期间最多持有 displayed + pending 两个资源。
3. 请求完成但未被组件认领的资源只获得短暂发布宽限期；组件切换或卸载继续显式立即 release。
4. 漏 release 或 WebView 异常不能永久占用 registry；活动图片通过短 lease 心跳可无限期查看。
5. entry、encoded memory、staged disk 和协议响应使用独立预算及诊断。
6. 容量错误不能伪装成 decoder/backend 失败，也不能触发不能解决容量问题的 HEIF fallback。

### 预算模型

把 `ResourceRegistry::new(max_entries, max_memory_bytes)` 重构为可注入的明确配置，示意：

```rust
struct ResourceRegistryLimits {
    max_entries: usize,
    max_encoded_bytes: usize,
    max_materialized_responses: usize,
    max_materialized_bytes: usize,
    publish_grace: Duration,
    ui_lease: Duration,
}
```

生产环境采用：

| 预算 | 已确认值 |
| --- | --- |
| registry entry | 512 项，作为泄漏保险；活动资源仍不可强制淘汰 |
| encoded resource 常驻内存 | `max(1 GiB, system_total_memory / 8)` |
| 系统内存探测失败 | 回退 1 GiB |
| 低于 8 GiB 的设备 | 按用户决定优先保证 1 GiB，允许超过物理内存的 1/8 |
| protocol materialization 并发 | 保持 4 个响应 |
| protocol materialization bytes | 与 registry 解耦并保持 128 MiB |
| 未认领资源发布宽限期 | 5 秒 |
| 已认领活动 UI lease | 30 秒 |
| 前端活动资源续租间隔 | 10 秒 |

动态值是允许占用的上限，不在启动时预分配。只让 encoded resource registry 使用动态内存预算；Tauri
`Response<Vec<u8>>` 的临时 materialization 继续使用独立的 4-response/128 MiB 限制，不能随机器内存
扩大到数 GiB。系统总内存只在初始化时探测一次，使用 checked arithmetic，并把 `u64` 到 `usize` 的
转换限制在当前平台可表示范围。实际实现若新增跨平台系统信息依赖，须同步 workspace 依赖和相关文档。

staged/managed/original 文件不计入 encoded bytes。entry 先统一提高到 512，并继续度量 staged 临时文件
的数量与磁盘字节；若压力测试证明需要单独背压，再增加 staged count/byte budget，而不能把文件大小错误
计入 encoded 常驻内存。

测试不通过 `cfg(test)` 暗中改变生产公式。单元和压力测试显式注入 limits；内存压力测试使用
128 MiB `max_encoded_bytes`，既保证边界可实际触发，也确保测试与生产运行同一套 reservation 代码。

### 实施步骤

#### 1. 先建立失败回归与可观测性

- 复现 64 个仍受保护的文件型资源导致第 65 个 staged HEIF publication 失败。
- 复现一个 loupe 同时持有 thumbnail、preview、full，以及 full 显示后低等级资源仍未释放。
- 记录 registry 当前 entries、encoded bytes、UI-leased、read-leased、released/expired 数量和本次请求量。
- 将统一 `ResourceBudgetExhausted` 诊断细分为 entry、encoded memory、materialized count/bytes；错误至少
  包含当前值、上限和请求值，便于确定是哪种预算耗尽。

#### 2. 收窄前端资源所有权

- `Thumbnail` 不再根据全部 thumbnail/preview/full projection 构造 lease 集合，只保留当前
  `displayedImage` 对应资源与尚未完成 load 的 `pendingSource` 对应资源。
- 新图完成浏览器 load 并提升为 displayed 后，立即 release 被替代的低等级资源，不等待组件卸载。
- 请求迟到、组件已 disposed、结果从未成为候选或被 revision/generation fence 拒绝时，调用
  `releaseUnretainedMediaResource`。
- 保留 `mediaResourceLease.ts` 的前端引用计数：Thumbnail 与 Loupe 共享同一 resource ID 时，只有最后
  一个 owner 离开才向 Rust release；继续覆盖 StrictMode 的同一 microtask 释放/重取。
- 组件卸载、切图和 projection replacement 继续立即 release，不把 30 秒 lease 当作正常回收路径。

#### 3. 拆分发布宽限期和活动 lease

- registry entry 初始进入 5 秒 `PublishedGrace`，只负责覆盖 Rust 返回、IPC 和 React 接管窗口。
- 前端首次成功 renew 后进入 30 秒 `ActiveUiLease`，每 10 秒续租；只要仍显示即可无限期查看。
- release 立即把 entry 放到 LRU 首位并允许淘汰；过期资源同样允许淘汰。
- 窗口后台导致 JS timer 被节流时，资源可以过期；回到前台后沿用现有 renew-false → refetch 恢复路径。
  managed cache 和源文件不因此丢失，不把恢复描述为零闪烁保证。

#### 4. 接入动态 encoded 内存预算

- 增加可独立测试的 `resource_memory_budget(total_memory)` 纯函数，规则固定为
  `max(1 GiB, total_memory / 8)`；探测失败返回 1 GiB。
- 生产 shared registry 初始化时一次性计算 limits；构造函数和测试替身继续支持显式注入。
- encoded `Arc<[u8]>` 按实际长度 reservation；file/staged payload 保持 0 encoded bytes。
- 协议 response 的 count/byte reservation 从 registry encoded 上限解耦，保持 4/128 MiB。
- entry 上限调整为 512；不通过扩大内存预算掩盖 entry 生命周期问题，也不强制淘汰 active UI/read lease。

#### 5. 修正 HEIF 错误与回退

- `heif_full_error_allows_fallback` 将 `ResourceBudgetExhausted` 与取消、cache generation/source revision
  fence 一样排除在 fallback 之外；容量不足时保留当前渐进 preview，不重复执行 8192px 解码/发布。
- 将 `full-detail HEIF decode failed` 改成能区分 decode 与 artifact publication 的日志；若错误来自
  publication，明确报告对应 entry/memory budget。
- `failed to publish HEIF full projection` 不再紧跟一次同因无效 fallback；前端保持已有低等级图片而非
  清空画面。

### 测试与验收

Rust 单元/压力测试至少覆盖：

1. 以 32 MiB 分块注册四个 encoded resource，达到注入的 128 MiB 上限；额外 1 byte 被拒绝。
2. release 并淘汰一个 32 MiB resource 后可再次注册，计数和字节 reservation 无泄漏。
3. staged/file resource 不增加 encoded bytes，但受独立 entry 上限约束。
4. 系统内存输入 4/8/16/32/64 GiB 时，预算分别为 1/1/2/4/8 GiB；探测失败为 1 GiB。
5. materialization 始终独立限制为 4 个并发、128 MiB，不继承生产动态预算。
6. 5 秒未认领资源可淘汰；30 秒活动 lease 受 renew 保护；release 立即允许淘汰；read lease 在协议读取
   结束前仍阻止删除或 staged → managed transition。
7. entry 满时先淘汰 released/expired LRU，绝不淘汰活动 UI/read lease。
8. `ResourceBudgetExhausted` 不允许 HEIF Full fallback，其余合法 backend 错误保持原回退顺序。

前端测试至少覆盖：

1. 渐进加载期间同时续租 displayed + pending。
2. pending load 成功后释放旧 thumbnail/preview，只保留提升后的资源。
3. full 已可见时不再因 projection 中仍存在低等级结果而续租它们。
4. Thumbnail/Loupe 共享 resource ID、StrictMode 重挂载、迟到结果、切图和 IPC teardown 失败。
5. renew 因后台超时返回 false 时重新取得当前进程 descriptor，且旧 stale URL 不被反复重试。

端到端压力场景至少覆盖连续滚动 500 张以上照片、高密度网格与最大 overscan、快速切换多张 HIF
loupe、thumbnail → preview → full 升级、cache clear/prune 与活动读取竞争。验证 registry 数量回落到
实际 displayed/pending 工作集附近，encoded 峰值不越过预算，且不再出现 HEIF Full 因 entry 耗尽失败。

实施后运行：

```bash
pnpm check
pnpm test
pnpm build
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

涉及实现预计集中在：

- `crates/oxy-media/src/publication.rs`
- `crates/oxy-media/src/error.rs`
- `crates/oxy-media/src/pipeline/dispatcher.rs`
- `apps/desktop/src/components/Thumbnail.tsx`
- `apps/desktop/src/components/HeifTileCanvas.tsx`
- `apps/desktop/src/lib/mediaResourceLease.ts`
- 对应 Rust/Vitest 测试及实施完成后的正式架构、media README、性能记录

### 交付顺序

1. 失败回归测试和预算分类诊断。
2. 前端只持有 displayed + pending，并及时释放迟到/被替代结果。
3. 5 秒发布宽限、30 秒活动 lease 和 10 秒续租。
4. limits 配置化、512 entry 与生产 `max(1 GiB, RAM / 8)` encoded 预算。
5. HEIF publication 容量错误不 fallback，并修正日志语义。
6. 128 MiB 注入压力测试、500+ 图片滚动/HIF loupe 压测和完整跨边界检查。

以下实施记录对应本节计划；问题描述保留修复前状态。


### 实施与本机验收记录（2026-09-09）

- registry 改为可注入 `ResourceRegistryLimits`，生产 512 项、encoded
  `max(1 GiB, RAM / 8)`；启动时探测一次系统内存，失败回退 1 GiB。file/staged 仍记 0 encoded bytes，
  协议 materialization 保持独立 4 个/128 MiB。诊断包含当前/峰值 entries、encoded bytes、UI/read lease、
  released/expired、staged count/bytes 和临时响应 reservation。
- 发布宽限 5 秒，UI lease 30 秒，前端每 10 秒续租。仅保留 displayed + pending，浏览器 load 后释放旧图，
  保留共享引用计数和 StrictMode 保护；迟到及 revision fence 拒绝的资源也释放。
- 过期句柄恢复时更新现有 projection revision，避免新 URL 被旧 revision fence 永久拒绝；同一有效句柄再次
  交给前端时刷新发布宽限，保留已持有的活动 lease。资源恢复的文件检查移出全局 projection 写锁。
- HIF Full 真正显示后卸载其底层低清 Thumbnail。切图取消尚未返回的 Full 请求，并以有界、短时记录覆盖
  cancel 先于队列 admission 的竞态，避免快速切图积压旧 Full 解码。过期 artifact 改由 tiles 恢复时，
  旧图保留到新画布完成后释放，这条逻辑不依赖 debug/perf 探针。
- 容量错误报告具体 budget/current/limit/requested，不再触发 HEIF 解码 fallback。
- 128 MiB 注入测试覆盖四个 32 MiB resource 达上限、额外 1 byte 拒绝、release 后恢复；另有 64 个活动
  file 阻止第 65 项的旧问题回归、512 项配置成功、600 个文件工作集、grace/renew/read lease、staged
  transition、动态内存预算和独立协议 reservation 测试。
- `pnpm check`、157 个前端测试、生产 build、`cargo fmt --all --check`、workspace Clippy（`-D warnings`）
  和 workspace tests 均通过。Rust 合计 516 passed / 15 ignored，包含真实 HIF/RAW 原生测试；ignored 是已有
  外部夹具或环境相关测试。原生测试在允许 macOS ImageIO 的环境运行。
- 高密度网格和列表各实际加载 600 张照片，entry 峰值分别 70/28，最终 40/19，与 DOM 显示数相同。
  80 张不同路径 HIF 快速切换后回访 8 张，三轮全部 Full 实际 load，entry 峰值 31/32/32、最终均 24；
  同时覆盖 cache clear/prune 与活动协议
  读取。没有预算耗尽，资源回落到实际工作集附近。JPEG/RAW/HIF 六个冷暖预览场景各三轮也通过
  800 ms 冷 / 150 ms 暖门禁。详见 `docs/PERFORMANCE.md` 的本次测量记录。

验收范围仍是 macOS 本机；Windows/Linux 原生文件锁和 packaged WebView 矩阵未运行，既有跨平台限制不变。
