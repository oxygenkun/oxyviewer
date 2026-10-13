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

重构仍在实施：部分用例协调留在 desktop commands，repo/schema 和应用类型尚未全部
分配到功能拥有方，旧入口与 schema 兼容路径尚未清除。详见
[实施清单](../tasks/core-layout-refactor.md)；包数量收敛不代表该清单已经完成。
