# 08：按源表示规划 thumbnail 与 full

状态：**已实现并完成 Windows 验收**。2026-09-12。
实测与未达预算项见[验收报告](../research/jpeg-thumbnail-scroll-2026-09-12.md)。

本设计细化 [ADR 0006](../adr/0006-semantic-render-level-graph.md) 的原生方法选择，
先用于大尺寸 JPEG，再逐步接入 RAW 和其他格式。
实施顺序及卡顿治理见 [任务计划](../tasks/jpg-quality-and-scroll-performance.md)。

实现入口：`oxy-metadata-parser::jpeg_preview` 解析 APP1/MPF，
`oxy-media/formats/jpeg.rs` 执行有界头部读取，`pipeline/jpeg/selection.rs` 判断内嵌候选资格，
`delivery.rs` 提供缓存和生产者共用的交付约束；`pipeline/jpeg.rs` 选择候选并执行回退。
`pipeline/jpeg_transform.rs` 负责共享 JPEG 编码，`pipeline/thumbnail.rs` 负责缓存派生与交付兜底。JPEG 小图使用现有静态 libjpeg-turbo 的 IDCT 缩放，
然后以质量 90 编码到最长边 512；PNG/WebP 使用保留 alpha 的小 PNG。
libjpeg 接收调用方指定的目标尺寸；转换预算和执行共用同一 `DecodePlan`。
HEIF 服务继续拥有 tile 适配、输出文件和发布生命周期。此次整理保留缓存策略字符串与序列化格式。
结构整理验证：Rust workspace 格式、Clippy、测试及前端 211 项测试、类型检查、生产构建通过。
Sony/Lumix 外部 JPEG 夹具的输出尺寸、读取量和解码像素与整理前一致；编码文件仅有生成的
sRGB ICC 创建时间不同。真实 HIF 系数拼接回归通过。本次未重复完整 NAS 滚动性能矩阵。
完整 JPEG 继续使用主图资源。媒体协议通过后台线程 materialize，主图 DOM 使用异步解码，
等待解码完成后才替换已有图像；这不改变 full 的质量定义。

当前限制：探测总预算 1 MiB，候选前缀最多 64 KiB、最多解码三个候选；超限或不能验证的候选
回退主图。JPEG 规范化目前接受 8-bit 灰度/RGB像素与 RGB ICC，不能确认的 ICC 不会被重新标为
sRGB。转换许可最多两个任务、合计估算 256 MiB；大图超过估算预算明确失败。
该许可覆盖新的 JPEG/PNG/WebP 小图规范化及已有大缓存的派生，不是整个进程或既有 RAW/HEIF
解码器的总内存上限。完整图像保留、原生 registry 和协议响应沿用各自独立预算。

## 1. 目标与现状

同一张照片的 `thumbnail` 和 `full` 表达不同质量需求。JPEG 与 RAW 遵循同样的请求、
选择、缓存和呈现流程；各格式对“完整质量”的定义仍由其策略决定。

现有前端 `renderPlan()` 已让 JPEG 和 RAW 分别请求 `generatedImage:thumbnail` 与
`generatedImage:full`。实施前的缺口在 `pipeline/dispatcher.rs`：JPEG 的所有等级均注册原图。
因此，只有前端两个不同 query key，并不能避免缩略图读取与解码整张原图；现在原生 thumbnail
路径通过表示选择与有界转换补齐这一层。

历史上的静态 planner 已被 dispatcher 取代，见 [07 的最终收敛](07-media-refactoring.md)。
这里新增的 planner 用于**基于实际源表示选择输入与转换步骤**，不是恢复另一份静态格式矩阵。

“高质量 JPG”不新增为 AssetKind，也不按文件体积或 Make 字符串判断。
所有 JPEG 进入相同策略；通过按需探测，判断主图是否已满足缩略图交付预算、有哪些有效内嵌图。
小 JPEG 可以复用原图，大 JPEG 必须选择或产生适用的小图。

## 2. 三层职责

```text
React renderPlan(kind, surface, platform)
  只决定语义请求及展示方式
          ↓
Tauri get_preview(path, level, priority, rank)
  沿用 projection、去重、调度、取消与版本控制
          ↓
oxy-media dispatcher → 请求策略 → 有效缓存 / 有界源探测
          ↓
纯函数 representation planner
  输入：请求约束、已验证候选、后端能力
  输出：直接交付 / 提取 / 转换 / 有序回退
          ↓
executor → 显示规范化 → v2 artifact / registry → projection
```

- planner 不打开文件、不解码、不访问 SQLite、不创建线程、不发布结果。
- JPEG/RAW 的容器知识提供候选；backend 执行具体操作；executor 承担 I/O、取消和失败回退。
- 源探测在实际媒体请求的后台执行阶段发生，绝不进入目录首屏摘要、列表分页或 React render。
- 当前范围不重写 HEIF 会话、RAW full lane 或现有统一队列。

## 3. 两类契约：质量与交付成本

仅判断“足够清晰”不够。全分辨率图片也足够清晰，但不适合成为整目录缩略图的浏览器输入。

内部设计使用下列概念，具体 Rust 字段可以在实现时收敛；不新增 IPC 请求层级：

| 内部概念 | 内容 |
| --- | --- |
| `RenderRequestPolicy` | semantic level、质量条件、呈现条件、交付上限、policy identity |
| `SourceRepresentation` | 原图/内嵌图/有效缓存；关联的主图；来源；像素尺寸；字节区间；方向、色彩、内容几何 |
| `RepresentationOrigin` | primary、EXIF IFD1、MPF preview、RAW embedded、已有 artifact 等来源证据 |
| `DeliveryLimits` | 允许交给浏览器的最大边、像素数、编码字节；与解码工作内存预算独立 |
| `RepresentationPlan` | 候选 ID、操作序列、输出契约、失败时下一候选；不包含图像字节 |

候选 ID 绑定 `SourceRevision` 与解析规则版本；区间使用经校验的绝对 `u64` offset/length，
不能以“第 2 张图片”或一个可复用 URL 作为跨源版本身份。

### 3.1 首批 JPEG 策略

| 请求 | 最终质量与交付 | 首选路径 | 回退 |
| --- | --- | --- | --- |
| `thumbnail` | 内容最长边目标 `min(512, 主图内容最长边)`；不放大小图冒充清晰度。编码画布最长边 ≤512、像素 ≤512²、编码体积 ≤2 MiB | 合格的小缓存；足够清晰且与主图对应的内嵌图，必要时在 native 缩小 | native 解码主图并缩小 |
| `full` | 当前 JPEG 主图的原始细节与呈现；不得以 MPF 预览替代 | 原图受控资源 | 不支持的呈现由合格 backend 转换；失败保留底图并报告错误 |
| 显式 `preview` | 本轮保留现有 JPEG profile 的 thumbnail 复用关系；不是一个新 4096 解码阶段 | 同 thumbnail | 同 thumbnail |

512 是当前 JPEG thumbnail **策略参数**，不进入语义枚举、组件分支或 IPC `maxSize`。
2 MiB 是拟定的交付保护上限，不是实测最优值；生成的小图及适用 ICC 元数据需受此上限约束。
编码体积超限时重新规范化有界元数据/产物，仍无法满足则返回明确错误，不退回大原图。
不为达到字节上限无限降低画质。

交付限制必须在两个维度同时成立：有效内容足够清晰，包含 padding 的实际画布也不超限。
如果 padding 使两者冲突，先依据已确认的 contentRect 裁去 padding，再缩放。

小 JPEG 若本身满足像素、体积和方向/色彩条件，可直接交付；其 full/thumbnail 允许共享
底层资源，但仍是两个独立语义 projection。新策略不要求每个等级都制造一份文件。

### 3.2 与 RAW 对齐的边界

对齐的是“等级 → 质量条件 → 合适源表示 → 必要转换 → 交付”，不是强制相同解码器或
完全相同 full 定义：

- JPEG full 必须对应主图；MPF 的 Large Thumbnail 即使很大也不能标成 full Satisfied。
- RAW full 继续沿用当前最大相机 JPEG / development 的既定策略及表示语义。
- RAW thumbnail 接入时，内嵌 1616/1920 像素 JPEG 作为转换输入，最终交付同样受缩略图上限约束。
- HEIF 的经识别 160 像素快速 thumbnail 属于已有独立策略。本轮不把它强制升级为 512，
  不改变 full tile/session 行为；未来接入时保留其资格规则和真实 fixture。

### 3.3 160×120 小图的处理决定

本轮 JPEG thumbnail 使用一次最终合格小图回复，不引入“160 首帧 → 512 升级”的新生命周期。
160×120 IFD1 图对于本批大主图不满足最终质量，MPF 预览是更合适的输入。

这是有意选择：当前 `folderThumbnailCache` 以源身份缓存后，会阻止相同源再次 capture；
`api.ts` 对 thumbnail interim 也没有沿用 full 的持续订阅处理。
直接先发布 160 并标记完成，会使整目录缓存钉住低清图；标记 Interim 又要求补齐订阅与替换。
未来如首图延迟实测需要该功能，应单独引入 quality/resource revision 比较、保留升级订阅、
旧 Blob 的延迟释放和单调质量替换测试，不新增一个 `tiny` 语义层级，也不把小图伪装为 Satisfied。

## 4. 有界探测与通用 JPEG 候选

本次实际核查：241 个 Sony JPEG 和 69 个 Panasonic JPEG 均有 160×120 IFD1 图，
MPF preview 分别为 1616×1080 与 1620×1080。全部文件的主图方向为 Rotate 270 CW。
这证明这两个目录具备提取条件，不代表所有相机或后期导出的 JPG 都有相同结构。

### 4.1 解析与读取

1. JPEG marker 扫描在 SOS 前读取必要的 APP1/APP2/SOF，跳过无关 payload，不扫描主图压缩流找 JPEG 魔数。
2. IFD1 解析 ThumbnailOffset/Length；MPF 解析 endian、索引、各 MP entry 的 type/flags/dependency。
3. 各 offset 相对于其所属 TIFF/MPF 数据基点计算，解析时统一转换为绝对区间；不能照搬 ExifTool
   已规范化的输出值作为规范中的相对 offset。
4. MPF 只接受类型及主图依赖关系明确的预览。排除 primary、自引用、stereo view、gain map、
   depth 等不同语义图像；不选择“文件里的第二个 JPEG”或“最大 JPEG”代替关联验证。
5. 通过 seek 读取所选候选的头部和字节范围；读取范围前后校验 SourceRevision。源变化时取消
   旧计划并重新 admission，不能把旧 offset 用在新文件上。

初始探测预算：读取缓冲 64 KiB，累计元数据 payload ≤1 MiB，marker ≤4096 个，MP entry
≤64 个；均用命名配置集中定义。超限记录 `probeBudgetExceeded`，走普通主图缩小路径。
元数据预算不等于所选图像的读取预算；候选图片还需通过独立编码大小和解码像素预算。
没有 MPF 是正常能力缺失，不是异常；负探测结果只能按源 revision 有界缓存。

检查 offset 加法溢出、越界、截断 IFD、循环引用、长度不一致、JPEG 格式、实际 SOF 尺寸、
解码失败。坏预览只淘汰该候选；主文件不可访问/源已变化/取消不得反复读取其他候选。
依赖正常 I/O 返回的取消不等于能强制中断 SMB 内核读；不得承诺取消时间硬上限。

### 4.2 模块归属与扩展点

| 位置（规划） | 职责 |
| --- | --- |
| `oxy-metadata-parser/src/jpeg/mpf.rs` 等纯解析模块 | 解析传入的 JPEG segment / TIFF 数据及索引，返回小型结构和受校验区间；不执行 NAS I/O |
| `oxy-media/src/formats/jpeg.rs` | 有界 marker/seek 读取、候选构建、容器关联验证 |
| `oxy-media/src/formats/jpeg/quirks.rs` | 仅放有 fixture 支持的方向/色彩/padding 等格式变体规则 |
| `oxy-media/src/delivery.rs` | 通用尺寸／编码字节约束与直接交付判断；缓存不依赖 pipeline |
| `oxy-media/src/pipeline/jpeg/selection.rs` | JPEG 内嵌候选的质量、几何、色彩与编辑状态判定 |
| `oxy-media/src/pipeline/jpeg_transform.rs` | 共享 JPEG 缩小／编码，返回字节、尺寸及呈现信息 |
| `oxy-media/src/pipeline/thumbnail.rs` | 读取缓存派生输入、适配缩略图交付并发布结果 |
| `oxy-media/src/backends/libjpeg.rs` 与 `backends/libjpeg/` | 参数化 IDCT 解码、系数拼接及安全 FFI；不拥有 HEIF 缓存或输出文件 |
| `oxy-media/src/pipeline/jpeg.rs` | 执行计划、尝试回退、复用 ArtifactCache 和 registry |
| 现有 `backends/`、`presentation.rs`、`cache/encode.rs` | 解码、方向、色彩、缩放、编码；按实际功能需要提取共享函数 |
| `oxy-fs` | 继续负责源身份与文件观察；不移入媒体质量规则 |

模块名为规划位置，不是当前已经存在的 API。优先复用现有 IFD 解析和 reader，避免第二份 TIFF parser。
不给每个 vendor 建 executor，不引入插件框架或巨型 decoder trait。新 vendor 若遵循标准 MPF，
通过候选资格验证即可走通用路径；有真实差异时补局部规则和 fixture。
RAW 后续把 LibRaw 的候选/执行能力接入同一契约；不在纯 planner 中调用 LibRaw。

后期编辑可能保留已经过期的内嵌预览，只有尺寸匹配不能证明内容一致。发现编辑历史、
几何/方向冲突或未经验证的呈现关联时，保守使用当前主图缩小；不能只凭 Make 放行。
对支持的相机原始 JPEG 做主图与预览的配对视觉验证；源 revision 只证明读取一致性，
不证明编辑软件维护了预览内容。

## 5. 选择顺序与执行

同一个源版本和请求策略下：

1. 查有效 projection / 小 artifact：只有质量、呈现和 DeliveryLimits 全部满足才能直接交付。
2. 已有高清 artifact 如果满足源/呈现条件，可作为 native 派生输入；不得直接返回给 thumbnail。
3. 必要时探测源候选，排除不符合内容/呈现关联的表示。
4. 优先满足质量的低成本候选：已经合格的小图优于需要缩小的内嵌图；合格内嵌图通常优于主图。
   同类候选按需要解码的像素与编码字节排序，选择结果确定；已知本地有效缓存可优先于 NAS 重读。
5. 执行提取、解码、应用方向/色彩、必要内容裁剪、等比缩放、编码；再验证实际输出。
6. 发布符合当前请求的资源。unsupported / malformed candidate 进入下一候选；取消、
   source stale、不可恢复 I/O 结束本次计划，不启动 expensive fallback。

planner 不保证任意输入都能生成 thumbnail：所有合格 backend 失败时返回错误，保留已显示
的旧像素，不把整个大 JPG 作为“应急 thumbnail”交给浏览器。

JPEG full 可以跳过 MPF 探测，直接按现有主图资源路径处理；不等待 thumbnail 或其派生锁。
thumbnail 与 full 仅共享不可变源事实/已完成 artifact，不强制共享一个取消 token。

## 6. 方向、色彩、几何与内存

- 提取图可能不带自身 Orientation，使用经过验证的外层方向，避免漏转或重复旋转。
  产生缩小 artifact 时在 native 应用全部 8 种 EXIF 方向并规范化方向元数据。
- 子图 ICC 优先；只有资格规则证明子图与主图使用相同色彩解释时才继承主图 ICC。
  显式 EXIF sRGB 可作为依据；不能给未知 RGB/CMYK/AdobeRGB 直接贴 sRGB 标签。
  转换使用合格 decoder 与已有 lcms2 能力；无法建立可靠子图色彩关系时回退主图。
- 重新编码的 JPEG 首版使用现有质量 90 路径，并写入实际输出 ICC。色彩转换后的 sRGB 与
  “保留相机编码字节”分别标注，不混用 provenance。已有足够小的合格图无需重复转码。
- `PreviewResult.width/height` 表示交付图像的实际尺寸；`geometry.displaySize` 表示源的逻辑显示尺寸，
  `contentRect` 表示交付图像中的内容区。缓存命中和首次生成必须给出同样几何。
- 不通过“黑色像素检测”自动裁图。只有经确认的 padding 规则允许裁剪；不确定就换候选或主图。
- 转换的临时解码内存按源候选像素、位深、输出与后端缓冲估算后预留；不能按 512 的输出尺寸估计大图输入成本。
  独立预算设计见任务计划，registry 编码字节限制不等于解码像素或 WebView 总内存上限。

## 7. 缓存、投影与浏览器保留

### 7.1 缓存替代规则

当前 `DetailRequirement::Display` 只有最小清晰度，`native_detail` 也可以满足它。
保留这种“可以作为输入”的能力；新增 planner 的交付校验，区分：

```text
质量/呈现成立 + 交付上限成立 → DeliveryReady
质量/呈现成立 + 交付上限不成立 → DerivationInput
源版本/呈现/内容关联不成立 → Reject
```

不能让现有 `ArtifactCache::lookup` 提前注册并回复超限资源。实现使用 `BoundedThumbnail`
约束正常 lookup，另查可派生的较大缓存，在派生完成后才发布 thumbnail。

新 JPEG thumbnail 使用独立 policy revision `jpeg-thumbnail-v1-bounded-mpf`；full 保持
原图 policy。`preview_policy_revision()` 必须区分等级，避免旧 SQLite Ready projection
绕过新 planner 继续返回大图。RAW thumbnail 已拆为 `raw-thumbnail-v1-bounded-jpeg`，preview/full
各自保留原策略身份。Ready 投影恢复还会检查实际尺寸和编码长度。

沿用 v2 的 SourceRevision、VariantIdentity、manifest 与 publication。输出 target 应编码
派生策略版本；表示来源、实际方向/色彩/contentRect 参与产物身份。现有 `Embedded` 枚举表示
来源于内嵌图，不保证字节原样提取；本次 MPF 小图明确是重新编码产物，诊断 backend 为
`jpeg-mpf-thumbnail`，主图派生为 `jpeg-primary-thumbnail`。小图永远不能满足 JPEG full 的主图要求。

旧高清 cache 不批量清空，可以作为派生输入；源/格式不相关的缓存不受影响。
原图不复制到 thumbnail cache；MPF 大预览首版按需提取后生成小 artifact，不强制同时永久保存两份。

### 7.2 必须保留的生命周期

按源+等级进行 admission 去重；源探测、缓存 I/O、编码和事件发送不持有全局队列/projection 锁。
生产锁后重查，clear generation fencing，descriptor 恢复乐观校验、active publication、lease
连续性、切目录与晚到结果校验均沿用现有契约，详见 [性能不变量](../PERFORMANCE_INVARIANTS.md)。

缩略图交付后，浏览器直接 fetch 小 JPEG Blob 并 decode、保留其 Image，不再走大图 Canvas/PNG
转换。可见 Thumbnail 与后台 preload 统一使用该保留入口；现有 onload capture 不再对同一小图
多做一次 Canvas 转码。浏览器副本就绪后释放 native lease，整目录保留不被通用 LRU 或虚拟卸载淘汰。

本次采用 native resize，未新增 Worker；浏览器拒绝未知超限输入，不能恢复主线程大图 toBlob。
PNG/WebP 在 native 生成保留 alpha 的小 PNG，既有 TIFF/HEIF 小图入口也必须满足交付边界。

## 8. 对前端和公共契约的影响

- 保持 `get_preview` 入参、`RenderLevel`、`PreviewResult` 及 `ImageProjection` 的语义。
- 前端不发送 vendor、EXIF offset 或像素选择；继续请求 thumbnail/full，Loupe 立即启动 full，
  只复用已经存在的 thumbnail 底图。
- 明确 JPEG profile 的命名与独立策略，补 JPEG grid/loupe 方法身份及资源选择回归；不为
  其他 vendor 复制 React 分支，也不改变浏览器 demo 的无原生环境行为。
- 性能诊断在 `PreviewDiagnostics` 中添加可选 `sourceReadBytes`、`sourceReadCalls`、`probeMs`，
  配合既有 backend/totalMs 区分提取路径与总耗时。新增序列化字段统一放 oxy-domain 并同步 TS。
  诊断按需采集、每请求汇总，不能逐步同步打印或同步写 localStorage。

## 9. 首批接受标准

1. 两组真实 JPEG 在冷 thumbnail 请求中只读取必要头部及合格内嵌段，交付图 ≤512；热请求不重读主图像素。
2. full 仍显示主图完整尺寸与细节；thumbnail artifact、MPF preview 不能误命中 full。
3. 无 MPF、坏 MPF、未知 vendor、普通小 JPEG、旋转/镜像、padding、ICC/色彩不明确与编辑过的 JPEG 均有明确路径。
4. cache hit、高清派生输入、旧 projection、跨实例恢复与切目录取消不绕过新交付限制。
5. 首图、整个目录预热、滚动和内存由真实 Release WebView 验收；不能用 native Ready 或请求返回代替小图已绘制。
6. 本文是设计，不是性能通过记录。实施与验证项均记录在任务计划中。
