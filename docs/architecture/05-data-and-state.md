# 05：状态、数据与安全边界

本章说明 OxyViewer 的状态存在哪里、哪些数据可重建、哪些写操作会影响用户文件，以及当前
安全边界的真实范围。

## 1. 四类状态

```mermaid
flowchart TD
    app["OxyViewer 状态"] --> uiState["UI 交互状态"]
    app --> requestState["请求与服务器状态"]
    app --> runtimeState["Rust 运行时状态"]
    app --> persistentState["本地持久状态"]

    uiState --> zustand["Zustand"]
    requestState --> reactQuery["React Query"]
    runtimeState --> appState["AppState services"]
    persistentState --> sqlite[("SQLite")]
    persistentState --> previews[("预览缓存")]
    persistentState --> xmp[("用户 XMP sidecar")]
```

把这四类混在一个 store 中会造成生命周期不清：例如清除 React Query 不应删除用户 XMP，
切换视图也不应重建 SQLite 连接。

## 2. Zustand：同步 UI 意图

`useWorkspaceStore` 保存：

- grid/list/loupe 视图与网格偏好；
- selected IDs 和 active ID；
- 左侧栏、Inspector、Settings、Navigator 的开关；
- locale 和硬件加速偏好；
- 搜索、类型过滤、排序和方向。

这些值由用户交互立即修改，主要决定“界面想显示什么”。它们不代表磁盘事实，也不保存预览
Promise。当前 store 没有持久化 middleware，应用重启后恢复默认值。

选择模型中，普通选择把数组替换为单个 ID；按 meta/ctrl 的 additive 选择切换成员，并把操作
对象设为 active。打开新目录时调用 `clearSelection`，避免旧 ID 指向新 session。

## 3. React Query：异步结果生命周期

React Query 保存通过 Tauri 或 demo API 得到的数据：

- assets 的多页结果；
- library roots；
- asset details；
- 各 stage preview result。

query key 是缓存身份的一部分。典型 key 包含 asset ID、修改时间、size/stage 和 priority。
修改时间变化会让旧预览 query 不再匹配；priority 变化允许建立高优先级请求。

React Query 管理 loading/error/retry/cache/abort lifecycle，但它不是 Rust 缓存的替代品。刷新
目录时要同时让 Rust snapshot 和相关 React Query 失效，否则任一层都可能继续返回旧数据。

## 4. Rust `AppState`：共享服务状态

`AppState` 在 Tauri setup 中创建一次。它持有：

- `FsCatalog` 的 folder sessions 与内存目录快照；
- `JobRegistry` 的作业取消 flags；
- `Library` 的 SQLite connection；
- preview cache directory；
- `HeifDecodeService` 当前 session、tiles 和 diagnostics。

这些状态只活在 Rust 进程内，除了明确写入 SQLite/文件的部分。关闭应用后 sessions、jobs、
目录快照和 HEIF tiles 全部消失。

## 5. SQLite 资料库

`oxy-library::Library::open` 在 app data 目录创建 `oxyviewer.sqlite`，启用 WAL，并确保三张逻辑
结构存在：

```mermaid
erDiagram
    LIBRARY_ROOTS {
        text path PK
        integer added_at
    }
    ASSETS {
        text path PK
        text name
        text kind
        integer modified_at_ms
        integer size_bytes
    }
    ASSET_SEARCH {
        text path
        text name
    }
```

Mermaid 图没有画关系线，因为当前 schema 没有外键，也没有代码维护 `assets` 与 FTS 表的同步
关系。当前已完整使用的是 `library_roots`：只有用户明确添加的 canonical path 才持久化。
`assets` 和 `asset_search` 为后续可重建索引打基础，完整索引流程尚未落地。

SQLite connection 放在 `Mutex` 内，因为 `rusqlite::Connection` 的访问需要串行化。WAL 改善
读写并存和崩溃恢复，但不会自动使单个 connection 并发执行。

## 6. 预览缓存

preview cache 位于 Tauri `app_cache_dir()/previews`。它满足：

- 删除不会损坏源照片；
- 下次请求可重新生成；
- key 包含源文件身份和 decoder version；
- 先写临时文件再原子持久化；
- 同 backend tag 的 4096 stage 可以满足 512 请求；HEIF 另有 full-cache 复用路径。

缓存清理/容量上限目前不是完整功能。未来清理必须只作用于 app cache 目录，绝不能按未校验
路径递归删除。

## 7. XMP sidecar：用户数据，不是缓存

`oxy-metadata` 对 RAW 的 rating/color 读写使用同名 XMP sidecar；更新时只替换
`rdf:Description` 上对应的 `xmp:Rating` / `xmp:Label` 属性，保留其他 XMP 字段。首次写入时
创建最小 Adobe 风格 XMP。JPEG/HEIF/HIF 则通过 ExifTool 更新容器内嵌 XMP。

这是一项持久用户写入。它与 preview cache 不同，不能随意删除。内嵌写入当前从 `PATH`
查找 `exiftool`，也可用 `OXY_EXIFTOOL_PATH` 指定；正式发行版仍需捆绑 worker。

`patch_metadata` 在 blocking worker 中处理多选路径，完成后使对应目录摘要缓存失效，并刷新
详情与列表查询。普通目录打开仍使用廉价分页；只有启用 rating/color 筛选时才批量读取整个
当前目录的元数据，然后进行过滤和分页。

## 8. 文件操作

`FileOperation` 是 tagged enum：Rename、Copy、Move、Trash。实际操作集中在 `oxy-fs`：

- rename 只接受单个 normal filename component，拒绝空值和路径穿越式名称；
- 目标存在时拒绝覆盖；
- copy/move 同步处理 XMP sidecar；
- trash 使用系统废纸篓，并同时处理 sidecar；
- 返回所有受影响路径。

```mermaid
flowchart TD
    operation["FileOperation"] --> operationKind{类型}
    operationKind -->|Rename| validateName["校验单一文件名"]
    operationKind -->|Copy| transfer["复制源与 sidecar"]
    operationKind -->|Move| move["移动源与 sidecar"]
    operationKind -->|Trash| trash["送入系统废纸篓"]
    validateName --> collision{目标已存在?}
    transfer --> collision
    move --> collision
    collision -->|是| reject["DestinationExists"]
    collision -->|否| affected["返回 affectedPaths"]
    trash --> affected
```

### 8.1 必须知道的当前限制

浏览 command 会验证路径位于 `FolderSession` 根目录内；`execute_file_operation` 当前没有接收
session ID，也没有对 source/destination 做同样的 root containment 校验。它依赖调用方提供
明确路径和平台 capability 策略。

因此应准确表述为“文件操作集中到 `oxy-fs`，并具备名称、覆盖、sidecar 和 trash 安全语义”，
而不是“所有写操作都被 folder session 沙箱限制”。如果未来开放更广泛 UI 写操作，应考虑把
session/root policy 显式加入 command 契约并增加符号链接测试。

## 9. 作业注册与取消

`oxy-runtime::JobRegistry` 为作业生成 `job-N`，保存 `Arc<AtomicBool>`，返回含 priority 的
`JobTicket`。`cancel(id)` 设置 flag；worker 必须主动调用 `ticket.is_cancelled()` 才会提前退出；
`finish(id)` 从 registry 删除。

这是“取消词汇”，不是线程池或完整 scheduler。当前统一预览使用自己的 queue/gate，HEIF 使用
自己的 session flag，metadata 使用 JobRegistry。长期方向应统一可观察性，但不能假设已有一个
全局 worker runtime 在自动执行和抢占所有任务。

## 10. 事实来源与恢复策略

| 数据 | 事实来源? | 可自动删除重建? | 典型恢复 |
| --- | --- | --- | --- |
| 原始照片 | 是 | 否 | 用户备份 |
| XMP sidecar | 是 | 否 | 用户备份 |
| 显式 library root | 用户配置 | 不应无故删除 | SQLite/配置恢复 |
| SQLite asset rows | 否 | 是 | 重新索引 |
| 生成预览文件 | 否 | 是 | 重新解码 |
| React Query cache | 否 | 是 | 重新 invoke |
| Rust directory snapshot | 否 | 是 | 重新 `read_dir` |
| HEIF tiles | 否 | 是 | 重启 session |

## 11. 本章检查点

- 搜索文本为什么属于 Zustand，而搜索结果属于 React Query？
- 删除 preview cache 会丢失什么，删除 XMP 又会丢失什么？
- SQLite 中已有 `assets` 表是否等于后台索引已经完成？
- `cancel_job` 为什么必须有 worker 主动检查才能生效？
- 当前文件写操作是否受 FolderSession root 限制？
- 为什么 sidecar 的 rename/copy/trash 必须与源照片一起设计？

下一章：[06：扩展、调试与验证](06-extension-guide.md)。
