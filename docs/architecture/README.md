# 架构专题索引

这个目录把 [OxyViewer 架构总览](../ARCHITECTURE.md) 中的关键路径拆成可独立阅读的专题。
每篇都采用“先解释概念，再跟踪真实代码，最后列出约束和检查点”的结构。

## 专题

1. [Rust 与 Tauri 运行时](01-rust-tauri-runtime.md)
   - Rust crate、所有权、`Arc`、锁、异步与阻塞任务
   - Tauri command、state、event 和自定义协议
   - 前后端契约如何对齐
2. [文件夹浏览与分页](02-folder-browsing.md)
   - `FolderSession` 安全边界
   - 非递归扫描、内存快照、过滤排序和 cursor 分页
   - React Query 无限分页与虚拟列表
3. [统一预览流水线](03-preview-pipeline.md)
   - 渐进预览、双层调度、缓存和格式分派
   - JPEG/PNG/WebP、RAW、HEIF、TIFF 的不同路径
   - 取消语义和性能取舍
4. [HEIF 渐进瓦片与完整 JPEG 缓存](04-heif-tile-session.md)
   - 当前会话协议、渐进显示与热缓存路径
   - event 只传元数据，自定义协议传 RGBA
   - `generation` 如何防止旧结果污染新选择
5. [状态、数据与安全边界](05-data-and-state.md)
   - React Query 与 Zustand 的分工
   - `AppState`、SQLite、预览缓存、XMP
   - 文件操作和路径安全
6. [扩展、调试与验证](06-extension-guide.md)
   - 新增 command、媒体格式或后台任务的步骤
   - 错误、测试、性能和文档检查清单

7. [oxy-media 重构规则与目标](07-media-refactoring.md)
   - 多平台、多格式和厂商兼容规则的目标边界
   - 分阶段迁移、行为不变量与验收记录（进行中）

## 快速查找表

| 概念 | Rust 实现 | TypeScript 实现 |
| --- | --- | --- |
| IPC 契约 | `crates/oxy-domain/src/lib.rs` | `apps/desktop/src/types.ts` |
| 文件夹会话 | `crates/oxy-fs/src/lib.rs` | `apps/desktop/src/lib/api.ts` |
| Command 实现/注册 | `apps/desktop/src-tauri/src/commands/*.rs` / `apps/desktop/src-tauri/src/lib.rs` | `invoke(...)` wrappers in `api.ts` |
| 分页请求 | `FsCatalog::list_assets` | `useInfiniteQuery` in `App.tsx` |
| 渲染等级 | `pipeline::dispatcher` / `oxy_media::preview` | `renderPlan` |
| 预览优先级 | `PreviewQueue` / `DecodeGate` | selection/visibility priority hints |
| 调试队列观测 | `get_debug_queue_snapshot` | debug-only, read-only scheduler snapshots; see [`DEBUG_QUEUE_OBSERVABILITY.md`](../DEBUG_QUEUE_OBSERVABILITY.md) |
| HEIF 完整图 | `start_heif_full` / `HeifDecodeService` | `HeifTileCanvas` 消费 Rust 决定的 artifact 或 tiles |
| 资源 projection 状态 | `MetadataQueue` / `PreviewQueue` / SQLite | Zustand 只读镜像 |
| 原生元数据解析 | `oxy-metadata-parser` + `oxy-metadata` | `AssetDetails` / projection types |
| UI 请求生命周期 | 不适用 | React Query |
| UI 交互状态 | 不适用 | Zustand `useWorkspaceStore` |
| 资料库 | `oxy-library::Library` | library query in `App.tsx` |

## 术语

| 术语 | 本文中的含义 |
| --- | --- |
| asset | 一张受支持的媒体文件，而不是文件内容本身 |
| summary | 列表首屏需要的便宜字段，如路径、名称、大小、修改时间 |
| details | 选中后才构造的尺寸、拍摄参数、焦点与可编辑 metadata 投影 |
| preview | 为 UI 生成或直接提供的可显示图片 |
| loupe | 放大镜/单图查看模式 |
| stage | 渐进预览的质量级别：512、4096、full |
| session | 带身份和生命周期的一次文件夹或 HEIF 解码上下文 |
| cache | 可删除、可从源照片重建的数据 |
| sidecar | 与 RAW 同名的 `.xmp` 辅助元数据文件 |
| native adapter | 对操作系统或 C/C++ 媒体库的 Rust 封装 |
