# oxy-media

`oxy-media` 是 OxyViewer 的媒体预览执行层。它负责把语义化的渲染请求转换为本地缓存
artifact，并管理 RAW/HEIF/TIFF 的解码后端、优先级、fallback、并发限制和 HEIF 渐进 tile
session。

本 crate 不负责目录扫描、资源分类、SQLite projection 状态或前端调度。调用者应先通过
`oxy-fs` 获得 `AssetKind`，再把具体资源交给这里处理。图片字节不会通过普通 JSON IPC 返回；
预览接口返回本地 artifact 路径，HEIF tile 则通过自定义协议读取。

更完整的架构背景见：

- [`../../docs/architecture/03-preview-pipeline.md`](../../docs/architecture/03-preview-pipeline.md)
- [`../../docs/adr/0005-unified-preview-pipeline.md`](../../docs/adr/0005-unified-preview-pipeline.md)
- [`../../docs/adr/0006-semantic-render-level-graph.md`](../../docs/adr/0006-semantic-render-level-graph.md)

## 公共接口

crate 根模块只提供稳定 API re-export；`backends`、`formats`、`pipeline` 和 `cache` 的内部实现
不应由其他 crate 直接调用。

### 统一预览

```rust
pub fn preview(
    path: &Path,
    cache_dir: &Path,
    level: RenderLevel,
    priority: PreviewPriority,
    kind: AssetKind,
    cancellation: &CancellationToken,
) -> Result<PreviewResult, MediaError>
```

这是普通预览请求的主入口，也是 Tauri preview worker 对可解码格式调用的唯一入口。

- `path`：源文件路径。
- `cache_dir`：应用拥有的扁平 preview cache 目录。
- `level`：语义等级 `Thumbnail | Preview | Full`；调用者不传具体像素尺寸。
- `priority`：domain/IPC 层的请求优先级；media 内部将其转换为 decode gate 等级。
- `kind`：调用者已识别的 `AssetKind`。
- `cancellation`：所有 consumer 离开后由 preview queue 触发的协作取消 token。
- 返回值：`oxy_domain::PreviewResult`，包含 artifact 路径、尺寸、表示种类、渲染等级和可选诊断。

当前语义映射：

| 格式 | Thumbnail | Preview | Full |
| --- | --- | --- | --- |
| JPEG/PNG/WebP 等普通栅格图 | 原文件 | 原文件 | 原文件 |
| RAW | 512 目标预览 | 4096 目标预览 | 近全尺寸相机 JPEG，或完整 development |
| HEIF/HIF | 160 快速表示或 512 decode | 160 快速表示或 4096 decode | 完整源图 JPEG |
| TIFF | macOS system preview 512 | macOS system preview 512 | macOS system preview 4096 |

Windows/Linux 当前没有 TIFF system-preview backend，会返回
`MediaError::NativeDecoderUnavailable`。

### 请求优先级

`preview(...)` 直接接收共享 domain 中的 `PreviewPriority`，调用者无需了解 media 内部的
decode gate 类型。内部映射为：

| `PreviewPriority` | 内部 gate 等级 |
| --- | --- |
| `Loupe` | foreground |
| `Visible` | visible |
| `Nearby` | background |
| `Preload` | background |

高优先级请求可以越过尚未开始的低优先级请求，但不会抢占已经运行的 decode。gate 等级是
`oxy-media` 的私有实现细节，不属于公共 API。

### 尺寸读取

```rust
pub fn dimensions(path: &Path, kind: AssetKind) -> Result<ImageDimensions, MediaError>
```

按调用者已经识别的 `AssetKind` 确定性地选择普通栅格、libheif 或 LibRaw，只读取尺寸，不生成预览。返回的
`ImageDimensions` 包含 `width` 和 `height`。

尺寸读取仍可能访问媒体容器，因此不要在打开目录时对所有文件同步调用；应放在 metadata/detail
后台工作中。

### HEIF 完整图 session

```rust
pub struct HeifDecodeService;
```

`HeifDecodeService` 用于完整 HEIF 的渐进 tile 发布。一个 service 同时只维护一个 active
session；开始新 session 会取消旧 session，并清空旧 tile。

主要方法：

| 方法 | 用途 |
| --- | --- |
| `capabilities()` | 返回当前平台可用/不可用的 HEIF backend 及加速信息 |
| `begin(...)` | 选择 backend、替换旧 session，并返回 `HeifDecodeSession` |
| `decode(...)` | 在 blocking worker 中执行 decode，通过回调发布 tile 和完成状态 |
| `tile(...)` | 按 session、generation 和坐标读取已发布的 `HeifTile` |
| `cancel(session_id)` | 协作取消当前匹配的 session |

`decode` 是阻塞调用，不应运行在 async/UI 线程。`session.id + generation` 共同防止旧 decode
向新选择发布迟到 tile。

```rust
let service = oxy_media::HeifDecodeService::default();
let session = service.begin(path, cache_dir, generation, display_sharpening)?;

let diagnostics = service.decode(
    &session,
    cache_dir,
    |tile_ready| publish_tile_event(tile_ready),
    |diagnostics| publish_complete_event(diagnostics),
)?;
```

内部平台默认 tile 大小为 Windows 1024、macOS/Linux 512。
`HeifTile` 的 payload 是显式的 `Rgba { stride, bytes }` 或 `Jpeg(bytes)`；这些
bytes 应通过 `oxy-media://` 协议提供，不应塞入 JSON IPC。

### HEIF cache 查询

内部的 `cached_heif_full` 只读查询会检查完整源 HEIF JPEG 是否已经存在，不会启动 decode。
它属于 HEIF pipeline 的实现细节，不是 crate 的公共 API。Tauri 的 `start_heif_full` 由 Rust
统一决定直接返回 artifact projection，还是返回 tile session；前端不再维护平台策略或单独查询
缓存。HEIF preview 统一通过语义化 `preview(...)` 入口请求。

### Cache 管理

```rust
pub fn preview_cache_usage(cache_dir: &Path) -> Result<CacheUsage, MediaError>;
pub fn prune_preview_cache(
    cache_dir: &Path,
    max_size_bytes: u64,
    protected_path: Option<&Path>,
) -> Result<CacheUsage, MediaError>;
pub fn clear_preview_cache(cache_dir: &Path) -> Result<CacheUsage, MediaError>;
```

- `preview_cache_usage`：统计 cache 目录第一层普通文件。
- `prune_preview_cache`：按修改时间从旧到新删除，直到满足容量限制；可保护当前刚返回的 artifact。
- `clear_preview_cache`：只删除目录第一层普通文件，不递归删除子目录。
- `CacheUsage` 返回 `size_bytes` 和 `file_count`。

Cache 是可重建数据。调用者负责选择应用专属目录，并避免把任意用户目录直接交给清理接口。

### 版本和错误

```rust
pub const fn preview_policy_revision(kind: AssetKind, level: RenderLevel) -> &'static str;
pub enum MediaError;
```

`preview_policy_revision` 是缓存键与持久化 image projection 共用的唯一策略版本来源。媒体行为
发生不兼容变化时，版本更新可防止 ready projection 指向旧策略 artifact。

`MediaError` 区分 IO/image 错误、decoder 不可用、LibRaw/libheif/native backend 失败、颜色
转换失败、取消、完整 backend 尝试链失败和 system preview 失败。调用者应单独识别
`MediaError::Cancelled`；它不是允许继续尝试低优先级 fallback 的普通 decode 失败。

## 普通预览调用流程

```mermaid
flowchart TD
    caller[PreviewQueue worker] --> api[oxy_media::preview]
    api --> dispatcher[pipeline::dispatcher]
    dispatcher --> raw[pipeline::raw]
    dispatcher --> heif[pipeline::heif::artifact]
    dispatcher --> system[pipeline::system]
    dispatcher --> original[返回原文件]
    heif --> heifProbe[按需有界探测快速表示]
    raw --> cache[(preview cache)]
    heifProbe --> cache
    heif --> cache
    system --> cache
    cache --> result[PreviewResult: path + dimensions + diagnostics]
```

执行阶段遵循以下顺序：

1. dispatcher 根据 `AssetKind + RenderLevel` 直接分派，不在 Tauri command 中复制格式分支。
2. HEIF thumbnail/preview executor 优先检查 embedded cache，再按需执行有界 probe；目录扫描阶段
   不探测媒体内容。
3. pipeline 先检查精确 cache，再尝试复用更高等级 cache。
4. 需要源 decode 时先进入可取消优先级 gate，再取得可取消同源文件锁；取得锁后重新检查 cache。
5. 新 artifact 在 cache 的 `.tmp` 子目录中编码/同步，取消检查通过后再原子提交。
6. 返回 artifact 路径；调用方再更新最近使用时间、projection 状态并异步触发容量清理。

特殊并发规则：

- RAW full development 使用独立锁，不占用普通 thumbnail/preview decode gate。
- HEIF 缩略预览和 tile session 在内存 decode 完成后，会在后续 JPEG encode/fsync 前释放
  decode permit；直接生成 full artifact 的 backend transcode 作为一次完整操作受 gate 保护。
- 同一源文件的并发请求通过 source lock 合并，等待者在真正解码前再次检查 cache。
- Sony HIF 已验证的 160px 内嵌 JPEG 不进入 HEVC decode gate。

## Fallback 流程

dispatcher 的跨格式 fallback 是：HEIF full artifact 失败后尝试 foreground 8192px HEIF preview，
避免放大镜完全空白。RAW 与 HEIF 各自的 backend executor 保留有序尝试诊断。

HEIF 内部 backend 顺序由 `pipeline::heif::backend` 决定，并记录 backend 尝试诊断。取消会立即终止
计划，不会被解释为“尝试更慢兼容 backend”的许可。

## HEIF session 调用流程

```mermaid
sequenceDiagram
    participant Tauri
    participant Service as HeifDecodeService
    participant Backend
    participant Protocol as oxy-media protocol
    participant Cache

    Tauri->>Service: begin(path, cache_dir, generation, display_sharpening)
    Service-->>Tauri: HeifDecodeSession
    Tauri->>Service: decode(session, cache_dir, callbacks)
    Service->>Backend: 按 backend plan 解码
    loop 每个 tile
        Backend-->>Service: RGBA/JPEG tile
        Service-->>Tauri: publish(HeifTilePublication)
        Tauri->>Protocol: 前端按 URL 请求 tile
        Protocol->>Service: tile(session, generation, x, y)
    end
    Service-->>Tauri: complete(HeifDiagnostics)
    Service->>Cache: 稳定选择后写完整源 JPEG
    Service->>Cache: cached_heif_full(...)
```

## 内部模块边界

| 模块 | 职责 |
| --- | --- |
| `pipeline::dispatcher` | `AssetKind + RenderLevel` 分派和跨格式 fallback |
| `pipeline::artifact` | 跨格式 decoded preview cache 复用和计时等小型共享机制 |
| `pipeline::raw` | RAW embedded/developed/full 流程及 backend 顺序 |
| `pipeline::heif` | HEIF 子管线门面，仅声明内部子模块 |
| `pipeline::heif::backend` | HEIF backend 探测、选择、fallback 和尝试诊断 |
| `pipeline::heif::artifact` | 单一 HEIF `preview` 执行函数、full artifact 和 source-JPEG cache |
| `pipeline::system` | 平台 system preview |
| `backends` | ImageIO、Core Image、WIC、FFmpeg、libheif、LibRaw 适配器 |
| `formats` | 文件格式知识和窄范围兼容规则，例如 Sony SHIF |
| `decode_control` | decode gate、RAW full 锁、HEIF cache-write 锁和同源锁 |
| `cache` | cache key、JPEG/bytes 编码、原子提交和容量管理 |
| `media_source` | 尺寸读取、JPEG 完整性检查和 `PreviewResult` 构造 |
| `presentation` | artifact 的方向、颜色和表示契约 |

新增格式时，应先扩展 domain `AssetKind` 和 dispatcher 策略，再添加 pipeline executor 与必要
backend；不要在 Tauri command 中增加新的格式 `match`。
