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

FileService 把文件操作放入 core 的受控 worker。每个文件动作先检查路径和 sidecar
冲突，再持久记录 `prepared`；文件动作完成后记录 `filesApplied` 和源/目标版本证明。
资料更新与删除恢复记录属于 file_action workflow 的同一事务，文件系统不属于该事务。
批量操作按文件提交，后续失败保留已完成项并说明完成数量。恢复记录是 user 表，
cache clear 不会删除。`list_file_operation_recovery` 可检查待恢复动作；
`recover_file_operation_state` 只重试 filesApplied 记录的资料提交，先重新验证文件证明。
prepared 或已改变的记录要求检查文件与 sidecar，不会自动重做移动或删除。
普通 transfer 针对资产文件；外部移动后的整个根目录使用已确认的 root recovery。
删除目录后按路径边界清理已知资产的标签与派生特征，人工人物事实仍保留。

资源投影持久化属于 `resources::projections`，SQL 在相邻的 `resources::repo`。
OxyApp 为 metadata、preview 和 browse 装配一个共享投影句柄；Library 不再提供
投影读取、版本分配或提交入口。投影模块决定事务与接受顺序，repo 不开启事务。
WAL projection reader 仍独立于浏览 reader 和 writer；源观察版本、validAt、
stateRevision、清理后的顺序 fence、重启时移除旧进程 descriptor 的规则继续成立。

BrowseSnapshots 拥有目录快照、校验与持久化 worker，语句在 `browse::repo`；当前
索引的资产读取委托给 library repository。OxyApp 装配共享快照和 foreground gate，
把句柄交给 browse、资料库用例和文件操作；Library 仅持有索引复用近期验证摘要所需的
内部引用，不再公开转发浏览或失效入口。根目录移除由 LibraryService 先失效，根目录
恢复 workflow 显式接收 browse 句柄。退出分别唤醒 foreground gate、停止并等待快照
持久化排空。浏览延迟 example 改为调用 OxyApp/BrowseService 的公开用例。

资料库索引政策在 `library::index`，语句、分页 SQL、FTS 转义和 row 适配在
`library::repo::index`。事务范围、扫描代次、近期快照复用、每批 256 项/8ms 的
writer 预算以及 fair unlock 仍由政策层控制；repo 不打开事务。事务内缓存的预编译
语句按原粒度复用，未修改资产只更新代次，不重复写 FTS。原 cache 包装目录已移除，
源审计同时限制索引 repository 写入 cache/marker 表和政策模块携带 SQL。

PersonService 拥有模型状态、下载与导入、operation 预留/取消、检测采用、聚类与
文件夹分析/分组编排。模型和分析 worker 使用 CoreRuntime/PersonOperations，停止后
不再预留新 operation；取消后仍等待原 worker 展开，才允许下一次运行。检测采用
继续校验源身份与当前 producer，失效检测不产生人工事实。DirectML 的真实文件夹
吞吐验证已从 desktop 测试移到 core，仍需实际模型与 release 环境并显式运行。

当前人物 workspace、照片摘要和人工标框用例也由 PersonService 提供。标框保存先观察
规范路径和稳定来源身份，再验证界面提供的廉价版本，保存前重新观察文件；People
继续校验几何与所属文件夹。停止后拒绝新标框写入。Tauri 命令只转交完整用例。

全局人物目录不再在读路径迁移旧 folder/historical 身份。旧迁移入口、映射表声明、
历史参考/标签回填和来源别名清理已移除；目录读取只使用当前全局身份记录。
人物与标签的来源查询分别在各自 repository，workflow 在事务内组合结果，保留
手工和 sidecar 来源。旧身份 API 与未使用前端组件仍待退役，不能据此宣布兼容清理完成。

当前前端只挂载 GlobalPeopleContext：旧人物标框、检测框、文件夹审阅/聚类组件、
Sidebar 后备分支及其专用测试已移除。模型控件只负责安装与任务状态，分组仍由全局
人物面板发起。旧 demo 过滤与来源状态、剩余旧 IPC/schema 尚在清理范围内。

storage 及表声明宏仅在 core 内部可见，桌面和外部调用方通过 OxyApp 装配服务，
不能自行打开数据库或取得连接。内存库、连接诊断和全库 cache clear 的入口仅供内部
测试使用；它们继续验证按声明逆序清理且保留 user/marker 表。产品设置中的预览清理
仍只清理当前版本的预览文件，未扩展为 SQLite 清理。

默认数据和缓存路径使用 `core-v2` 子目录，custom cache 也有独立的版本子目录。
Store 只接收本核心的 application ID/schema version，或尚未创建表的空文件。
打开已有文件先做只读检查，未识别的旧库或未来版本不会被升级、重建或 checkpoint。
创建当前 schema 和版本标记在一个事务中完成；各功能负责自己的 indexes、FTS 和
cache-format marker。缓存格式失配只重建该功能声明的派生表，保留用户事实。
旧列迁移、旧标签来源回填和平铺缓存清理已移除。退出排空已接受的目录快照写入。

重构仍在实施：部分用例协调留在 desktop commands，浏览和索引中的 SQL 仍待进一步
收敛，旧人物入口与身份兼容路径尚未清除。详见
[实施清单](../tasks/core-layout-refactor.md)；包数量收敛不代表该清单已经完成。
