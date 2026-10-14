# 05：状态、数据与安全边界

状态归属与 package 图见[架构总览](../ARCHITECTURE.md)和[核心生命周期](11-core-ownership.md)。
本章区分界面意图、请求结果、进程资源、持久用户事实及可重建缓存。

## 界面、请求与权威状态

Zustand 保存 selection、grid/loupe、搜索/排序和面板偏好。React Query 保存请求的
loading/error/retry/abort 生命周期与分页响应。它们不决定哪份媒体或 metadata 结果有效。
Rust projection 持有单调 stateRevision，前端只镜像被接受的版本。刷新同时失效 Rust
投影与界面镜像，旧请求不能覆盖新选择；最近成功查询不跨目录显示。

OxyApp 装配一次共享 Store、功能服务和资源状态；Tauri AppState 只接入该宿主。文件会话
属于 files，目录快照和树调度属于 browse，索引和根目录属于 library。媒体 registry、
resource ID、lease 和 HEIF session 是进程资源，不能在重启后复用旧 URL。

持久 image projection 剥离旧进程 descriptor。恢复时校验 stable file/artifact，再注册
当前进程资源。坏 DTO 按未命中处理，继续保留版本 fence，不能让旧结果反向覆盖。

## SQLite 和拥有方

单个 SQLite 使用共享 writer、独立 browsing WAL reader 和 projection WAL reader。
读者不获取 writer 锁；相关计数、完成标记和结果使用同一读取事务。内存测试库使用单连接。
索引每批最多 256 项/8ms 软时间片，提交后公平交接；单条 SQL/commit 不能中途抢占。

storage 是私有机制模块：连接、WAL、表声明宏、schema registry 和版本检查。各功能的
schema/repo 跟随 owning feature。Repo 接收连接/事务执行语句，按列名读取，不开事务、
不判断业务规则。Feature 政策决定单功能事务；跨功能动作进入 app::workflows。

| 类别 | 当前含义 | 清理权限 |
| --- | --- | --- |
| user | 根目录、姓名、人工 tuple、审阅/排除、参考、事件、标签和文件恢复 | cache clear/重建不可删除 |
| cache | 索引、检测、向量、运行账本、建议、快照、投影 | 按 owning declaration 重建 |
| marker | cache format 或单调 allocator | 清缓存保留标记语义 |

每张表在 owning schema 声明一次，创建与清理由同一声明生成；registry 保持依赖顺序，
清理反序。外部创建的 FTS 用 external 声明。不能按表名前缀删除，不能通过删库清缓存。
产品的 preview-clear 只处理当前预览文件，并取消/失效相关请求。

当前数据、模型、配置及预览路径使用 core-v2 namespace，SQLite schema 为 2。
未知/旧/未来非空库先只读验证，拒绝时不打开 writer、不 checkpoint、不迁移或删除。
本轮已移除旧 folder/historical 用户表声明；版本 1 的中间库需单独保留，用新目录验收。
未来升级要明确保护用户声明，不能从“可重建资料库”推导用户事实可丢弃。

## 人物、标签与共同提交

PersonService 使用全局身份；tuple 是照片中的一个人脸/身体实例。同图不同人有独立
决定和参考。匿名分组、检索建议、检测和向量不能自动确立人工身份。旧 folder identity、
history linking、adoption 和兼容命令已经退役，姓名相同也不会隐式合并身份。

person_review、person_geometry 和 person_tags workflows 共同提交版本/幂等校验、
人工变更、参考撤回、人物标签来源、有效标签、持久 XMP 队列和 payload/result 历史。
人物政策返回受影响身份，不直接协调 tags。最终事件失败时全部回滚，重试只重放结果。

manual、sidecar 和 global person 来源独立。最后一个有效 belongs 撤回后才移除该人物
贡献；手工来源仍在。几何变化撤回失效参考并重新核对，明确否定仍被保护。成功提交后
PersonService 唤醒已有 coalescing TagXmpSync；失败不发成功状态，镜像失败留在持久队列。
图库历史可离线读取，不要求照片仍存在；清缓存不会删除姓名、决定、参考或事件。

## 元数据和文件变更

Media 负责 native 标签、方向、尺寸和 capture/focus 归一化；core 负责应用 metadata
解释、同名 sidecar、字段编辑和提供者任务。同名有效 XMP 对显示字段优先；缺失值不
擦除文件值。显式 embedded 写入继续跳过 RAW。部分批量成功必须刷新已写文件。

FileService 统一显式 rename/copy/move/trash/permanent-delete。路径、名称、覆盖冲突、
sidecar 和 symlink 语义由 files 检查；这是安全操作边界，不代表 OS 级目录沙箱。
调用涉及用户文件，不能把 SQLite cache policy 当成删除用户文件的许可。

文件系统和 SQLite 不同事务。每文件先记录 prepared，动作完成后记录 filesApplied 和
PathProof；资料提交后才删除恢复记录。未知文件动作不得自动重做；DB-only recovery
重验目标证明，部分批量成功保留。恢复记录是 user，清缓存保留它。

Same-folder rename 用目标 observation/path rebase 证明重绑强源身份，保留匹配事实、
参考和贡献；不匹配事实留存并标记 needsReview，手工标签保留。显式 root recovery
重新规划/校验证明，共同重绑记录及全局贡献，提交后产物复用失败独立计数。

## 调度和观察

后台重工作不在 UI/async 线程。CoreRuntime 拒绝停止后的准入和事件，取消协作任务并
等待真实 worker 退出；native 超时不等于 joined。目录快照有独立排空机制。
ExifTool 安装/配置与人物模型/分析由 core 追踪，等待安装 gate 的动作停止后不开始。

QueueDiagnostics 非阻塞读取五组队列并保留 busy 样本，不扫描目录、不触发 decode。
队列锁不包住 filesystem/SQLite I/O。详情见[性能不变量](../PERFORMANCE_INVARIANTS.md)。

下一章：[扩展、调试与验证](06-extension-guide.md)。
