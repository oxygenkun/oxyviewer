# 06：扩展、调试与验证

从真实状态和用例拥有方开始。应用功能在 core 的模块里，媒体在 media，纯解析在 parser。
当前地图见[架构总览](../ARCHITECTURE.md)，规则见[AGENTS](../../AGENTS.md)。

## 选择位置

| 改动 | 首选位置 |
| --- | --- |
| 界面/交互/显示派生模型 | apps/desktop/src |
| 目录分页、查询组合、树/快照 | core::browse |
| 安全文件动作与恢复 | core::files / app::workflows::file_action |
| 根目录和索引 | core::library |
| 标签层级、赋值来源、XMP 镜像 | core::tags |
| 人工人物规则、模型和分析 | core::people |
| 应用 metadata 编辑/提供者任务 | core::metadata |
| 请求/投影/缓存协调 | core::resources |
| native 读取、解码、像素/产物 | oxy-media |
| 容器/EXIF/XMP 等纯解析 | oxy-metadata-parser |

简单逻辑先用现有模块。没有独立复用、方向限制或构建测量，不为一个应用功能新建 crate。
Tauri command 不持有队列实现、不排序组合多功能过滤、不决定业务事务。

## 新增用例与 IPC

1. 在 owning feature 定义请求/输出和明确错误语义，camelCase serde。
2. 在 feature 政策里实现规则；多功能写入由具名 workflow 共同提交。
3. Repo 只拥有语句，schema 声明表与 user/cache/marker；不要让 storage 判断业务。
4. 增加完整服务入口，阻塞/CPU 工作放到受控 worker，明确停止/取消/迟到处理。
5. 增加 thin Tauri wrapper 和 generate_handler 注册；事件/协议由 adapter 处理。
6. 扩展 contracts 显式 exporter，运行 pnpm contracts:generate/check。
7. 更新 api.ts 和 browser demo；前端派生类型引用生成 DTO，不手写第二套 IPC 定义。
8. 添加有价值的行为/集成测试，运行 CONTRIBUTING 的适用矩阵。

图片字节不能通过 JSON IPC。使用路径、自定义协议、稳定 artifact 和 media leases。
新增契约字段要核对 null/缺省与 demo，不仅看 TypeScript 能否编译。

## 数据变更

每张表只在 owning schema 声明一次，按创建依赖注册；cache clear 反序执行，只接触
cache 类。Repo 不开启事务，读取 row 按列名。跨功能查询可以使用明确只读 projection，
跨功能写入调用 owning operations，不重建一个万能 cross.rs。

单库共同事务包括所需 revision、幂等 payload/result、持久镜像任务；提交后才通知、
失效和调度。文件操作另外记录 prepared/filesApplied、PathProof 和恢复边界，不能宣称
filesystem+SQLite 是原子事务。清缓存、重新索引/分析不能删除人工事实。

当前 schema 2/core-v2 拒绝旧库，不能静默迁移或重建用户库。未来 schema 升级必须明确
保护当前 user 声明并补充故障/重启保留回归。原生依赖及 notices 继续按 pinned 输入管理。

## 新格式或媒体规则

AssetKind、尺寸、源事实和 PreviewResult 属于 media；分析输入需求也由 media 定义。
扩展名分类使用 media::types::AssetKind，并由 core::files 统一发现。纯解析在 parser，
表示规划、backend 和 artifact 处理在 media；core 只校验应用路径/会话和请求意图。

不要为统一接口强制 UI 经 RGB8；模型分析可以按明确 AnalysisRequirement 获取像素。
保持方向、ICC、content geometry、渐进质量、fallback、generation 和 lease 语义。
缓存身份变化需要明确 schema/format 版本，不复用失效产物。

## 检查与证据

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --features oxy-core/contracts -- -D warnings
cargo test --workspace --features oxy-core/contracts
pnpm contracts:check
pnpm check
pnpm test
pnpm build
```

具体前提见[CONTRIBUTING](../../CONTRIBUTING.md)。Parser 可以独立测；media 测试无需
数据库或 Tauri，但仍需要 pinned native libraries。Headless core 测试不启动 WebView。
不要用镜像实现的测试代替规则和失败路径测试：验证回滚、重放、保留、取消或迟到等结果。

浏览/media 性能还需 `pnpm tauri build --no-bundle` 和代表真实 fixture，比较首屏、
冷/热预览、滚动 readiness、内存和后台工作。生成 synthetic fixture 与真实照片要区分；
本机 Windows 通过不能宣称 Linux/macOS 已验收。验证实例结束后关闭。

更新 docs/README 选中的当前合同；dated measurements 留在 research，完成/取代的计划
进入 archive。保留未涉及的 worktree 修改，按功能内聚显式暂存与 Conventional Commits。
