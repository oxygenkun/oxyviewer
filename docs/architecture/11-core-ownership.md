# 应用核心的装配、媒体读取与 worker 生命周期

当前 workspace 有四个 Rust package：桌面适配、`oxy-core`、`oxy-media`、
`oxy-metadata-parser`。应用功能在 core 内按模块组织；media 只依赖 parser，
core 依赖 media，桌面通过 core 调用应用能力，并直接使用媒体类型适配二进制协议。

`OxyApp::open(AppPaths)` 打开一个 Store，装配 Library、Tags、People、元数据提供者、
外部应用配置和资源队列。桌面 setup 负责从 Tauri 获取目录，把路径交给 OxyApp，
再注册事件适配和窗口。core 不持有 `AppHandle`、WebView 或 Tauri runtime。

媒体层负责 native 标签读取、capture 归一化、focus 几何和 XMP 解读。core 的
MetadataFacade 负责应用观察版本、同名 sidecar 优先级、字段编辑和失效；BurstScanner
从媒体层读取廉价 capture summary，不为连拍判断构造完整图片详情。

`CoreRuntime` 登记后台 worker、发布类型化 `CoreEvent`，并拒绝停止之后的新工作和事件。
`resources` 拥有 preview、metadata、目录树和索引队列以及缓存容量维护。桌面事件线程
只把枚举映射为 IPC topic，收到 Shutdown 后退出，宿主显式 join。

退出时取消待执行和活动解码，清空待执行 session，释放请求等待者，唤醒空队列和
foreground admission，并取消人物 operation。目录快照持久化也有独立的受控 lifetime。
OxyApp 等待 worker 退出；已经进入不可中断 native 调用的工作必须自行展开，超时返回
false，不能把超时当作线程已经终止。缓存维护的五秒间隔可以被停止信号立即唤醒。

元数据入队使用同一请求的 admission gate 保证合并正确性，SQLite 操作在队列锁之外执行。
I/O 完成后重新检查目录、generation 和停止状态。停止不会清空用户数据；cache/user/marker
表分类、projection revision 和媒体 lease 规则继续有效。

实现入口：`crates/oxy-core/src/app.rs`、`runtime.rs`、`resources/`、
`crates/oxy-media/src/metadata/`、`apps/desktop/src-tauri/src/events.rs`。
headless 测试覆盖装配与退出、人物取消、待执行和活动 preview 等待者释放，以及
SQLite admission 被阻塞时仍可观察队列并停止。

应用类型已移到功能的 types 模块，contracts 只导出对外契约。repository 也跟随
Library、Tags 和 People；跨功能资产移动、删除、人物标签贡献及根目录恢复进入具名
workflow。48 个表声明在功能的 schema 模块中，storage 汇总它们的分类与创建/清理顺序。

序列化契约通过固定版本的 [ts-rs](https://docs.rs/ts-rs/12.0.1/ts_rs/) 生成
`apps/desktop/src/generated/contracts.ts`，由 `pnpm contracts:check` 检查漂移。
frontend types 仅保留由生成类型派生的界面模型，例如 resolved URL 和本地树的根键。
原始 IPC 的 null 由适配层明确处理；metadata 的空值在显示投影中按缺省值比较，
继续保留可见字段未变化时的 asset 对象身份。

BrowseService 负责打开、分页、条件组合、目录树、切目录取消和刷新失效，桌面命令只把
整个同步用例放到 blocking executor。快照的后台校验使用 CoreRuntime，浏览进度也通过
类型化事件转发。TagXmpSync 用单个合并 worker 处理持久镜像队列，锁只保护 running/
requested 状态，不包住文件或数据库 I/O。失败项留在队列里供后续重试，停止拒绝新调度。

LibraryService 负责根目录配置、恢复预览、cache proof 收集、产物复用和延后索引。
根目录恢复 workflow 自己持有事务，提交前重新观察文件版本；验证失败时用户记录
全部回滚。产物复用在提交之后进行，失败单独计数。desktop 命令只适配参数、结果
和 blocking executor，Library 不再公开转发恢复入口。

MetadataService 负责详情、摘要请求和显式编辑。编辑 worker 由 CoreRuntime 登记，
逐文件检查取消与停止状态，JobRegistry 条目在成功、失败、拒绝启动或展开时释放。
批量编辑部分成功后仍刷新已写入的文件；后续文件失败不影响其缓存失效。
embedded 写入继续跳过 RAW，用户明确请求的 embedded 写入才调用 ExifTool。

默认数据和缓存路径使用 `core-v2` 子目录，custom cache 也有独立的版本子目录。
Store 只接收本核心的 application ID/schema version，或尚未创建表的空文件。
打开已有文件先做只读检查，未识别的旧库或未来版本不会被升级、重建或 checkpoint。
创建当前 schema 和版本标记在一个事务中完成；各功能负责自己的 indexes、FTS 和
cache-format marker。缓存格式失配只重建该功能声明的派生表，保留用户事实。
旧列迁移、旧标签来源回填和平铺缓存清理已移除。退出排空已接受的目录快照写入。

重构仍在实施：部分用例协调留在 desktop commands，浏览和索引中的 SQL 仍待进一步
收敛，旧人物入口与身份兼容路径尚未清除。详见
[实施清单](../tasks/core-layout-refactor.md)；包数量收敛不代表该清单已经完成。
