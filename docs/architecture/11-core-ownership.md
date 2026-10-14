# 应用核心归属与生命周期

当前 package 图为 desktop → core → media → parser；desktop 还直接使用 media 的
协议与媒体类型。应用功能以模块组织，旧 application package 和 forwarding API 已退役。
架构总览见[ARCHITECTURE](../ARCHITECTURE.md)，验收见[完成报告](../research/2026-10-14-core-layout-refactor-validation.md)。

## 装配和接口

OxyApp 打开一个私有 Store，装配共享的快照、投影、foreground gate、资源队列和功能
服务。桌面只提供平台路径、窗口、commands/events/protocols 适配。没有 AppHandle 或
Tauri runtime 进入 core。宿主不暴露原始 Library/People、queue 或数据库句柄。

公开用例入口是 BrowseService、LibraryService、FileService、PersonService、
MetadataService 和 ResourceService；Tags 提供自己的词汇/赋值行为。所有跨功能写入
由 app::workflows 命名，repo 保持事务内语句的归属，不自己打开事务或决定规则。

## 规则、行和事务

| 功能 | 规则/状态 | Repository/schema |
| --- | --- | --- |
| browse | 查询组合、快照校验/持久化、树调度 | browse::repo/schema |
| library | 根目录、索引 generation、256 项/8ms writer 预算 | library::repo/schema |
| tags | 层级、独立来源、有效赋值、镜像队列 | tags::repo/schema |
| people | 全局身份、tuple、决定、参考、机器证据 | people::repo/schema |
| resources | 投影接受顺序、请求/缓存 generation、失效 | resources::repo/schema |
| files | 动作、恢复阶段与来源证明 | files::repo/schema |

Storage 只拥有连接、独立 WAL readers、表声明宏、registry 和格式检查。表各自声明
user/cache/marker；创建按依赖顺序，清缓存反序。不存在按名称猜清理归属的路径。
当前 schema 2 不再声明旧 folder/historical 模型的 13 张表。姓名、人工框、全局决定、
参考、事件、标签、根目录和恢复记录仍为 user；索引、检测、向量和建议仍为 cache。
core-v2 隔离状态，旧/未来格式在 writer 打开前只读拒绝；没有自动迁移或删库。

人物 geometry、review 和 tag workflows 把人物变更、参考撤回、来源贡献、持久镜像
任务与请求结果放在一个事务里。人物功能返回受影响身份，workflow 调用标签拥有方。
手工/sidecar 来源独立，最后一个有效 belongs 撤回后人物贡献才撤回。提交失败不唤醒
镜像；成功或重放提交后 PersonService 唤醒已有合并 worker。

文件动作先记录 prepared，动作成功后记录 filesApplied/PathProof；资料提交与恢复
记录删除共同提交。失败保留已完成项，DB-only retry 校验证明，不重做未知文件动作。
同文件夹 rename 先观察目标，使用 path rebase 证明重绑强源身份；未匹配事实保留并
标记需核对，参考/人物贡献不能伪装有效。根恢复使用相同证明并共同更新全局贡献。

## 队列和媒体

索引调度/进度在 library::scheduling，目录树调度在 browse::tree_queue。预览和
metadata 队列、投影和缓存维护在 resources。QueueDiagnostics 使用 try_lock 读取
五组队列，busy 时保留先前样本并标记 stale；没有为观察而扫描目录或加载媒体。

Full selection 在 blocking 派发前捕获；准备不读源文件。ResourceService 拥有普通
预览范围、初始投影、缓存清理失效，以及 HEIF 缓存/产物/会话选择和 native 回调。
CoreEvent 携带 tile/status 数据，desktop 构造协议 URL；成功后只查缓存，不触发二次解码。
RAW retry、schedule keys 和取消也由服务拥有。

Media 拥有 SourceRevision、DecodeControl、native gates、表示规划、产物、registry
与 lease。Native 元数据和 capture/focus 归一化在 media；应用侧 MetadataFacade
负责观察版本、同名 sidecar 优先级、字段编辑和失效。重构保持队列算法、容量、优先级、
同源 admission、取消、提级保留等待者、generation、租约和迟到结果拒绝语义。

Metadata admission 的数据库工作在 queue 锁外执行，I/O 后再查停止/目录/generation。
索引仍复用近期验证摘要和缓存预编译语句；未修改资产只写 generation，不重复写 FTS。
独立 projection reader 不与浏览 reader/writer 合并成一个全局锁。

## Worker 和停止

CoreRuntime 登记队列、分析、文件、metadata 编辑和提供者 worker，非阻塞发布类型化
事件。停止拒绝 admission/publication、取消 cooperative/native session 工作、释放
waiters、唤醒空队列与 foreground gates。桌面事件 adapter 收到 Shutdown 后退出并 join。

BrowseSnapshots 另拥有持久化 worker，排空已接受写入并 join。Metadata 编辑逐文件
检查取消，部分成功仍刷新。ExifTool 配置/安装串行操作共享目标，状态观察独立；等待
安装 gate 的 worker 停止后不开始下载。人物取消保留 operation 独占直到 worker 退出。
已经进入 native 调用的工作自行展开，deadline 到达报告 timeout，不伪称线程已终止。

## 契约和证据

Feature-owned serde 类型由 contracts 显式导出，固定 ts-rs 生成 TypeScript，CI 检查
漂移。前端仅保留 resolved URL、paging 等界面派生类型。Demo 只维护当前全局人物命令。

Headless 回归覆盖廉价分页、查询交集、用户事实保留、同库回滚/重放、重命名强源证明、
根恢复、队列准入被 SQLite 阻塞、selection 顺序、停止与 native HEIF 回调/热产物。
真实 JPEG 回归覆盖人物贡献进入 XMP。正式 release WebView 和跨平台/真实照片吞吐
提供额外证据，不能从 headless 构建成功推断。当前限制见实施清单与 dated reports。
