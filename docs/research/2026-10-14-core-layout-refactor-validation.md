# 四包核心重构：实施与验收

日期：2026-10-14。分支：`codex/core-layout-refactor`。
基线：`595a46b`（已审查并提交的 repeat-last-tag 功能）。
状态：实施、正式媒体验收、分析对照、增量构建测量和最终检查全部完成。

## 已实现的组织

当前 Rust workspace 只有 desktop、oxy-core、oxy-media、oxy-metadata-parser。
Core 以 app/workflows、browse、files、library、tags、people、metadata、resources、
runtime/jobs 和私有 storage 组织应用行为。Media 自有媒体类型、原生读取、解码、
像素、产物和租约，只依赖 parser；parser 保持独立。

OxyApp 打开一个 Store 并装配服务。Tauri 适配命令、事件、协议和窗口，不持有 queue
实现或业务事务。Feature repo/schema 相邻，具名 workflow 拥有人物审阅/标框/标签、
文件动作与根恢复的跨功能事务。Worker 准入、取消、等待者和退出属于 core。

旧应用包、forwarding、folder/historical 人物 API、demo/过滤、映射与旧表声明已退役。
当前用户事实使用全局人物模型，数据/缓存隔离在 core-v2，schema 为 2。未知/旧/未来
非空库只读拒绝；不自动迁移、重建或删除。13 张旧表声明的切换得到用户明确批准。

契约由 feature/media 定义，经显式固定版本 ts-rs exporter 生成 TypeScript。
AGENTS、贡献指南、架构与人物专题已重写，旧 store/domain 拆分计划归档为历史。
当前规则见 [架构总览](../ARCHITECTURE.md)和[核心归属](../architecture/11-core-ownership.md)。

## 有价值的回归证据

- 真实文件 rename 的强源身份检查发现并修复旧路径 hash 未重绑的问题：匹配事实/参考
  保留，变化来源保留人工事实并标记复核；人物贡献撤回，手工来源保留。
- geometry/review/tag 最终事件故障共同回滚与幂等重放；同图独立人物决定和最后有效
  实例的贡献规则；真实 JPEG 确认后持久 XMP mirror 被 core worker 唤醒。
- 根恢复与来源变化/缺失、缓存重建/清理保留全部当前 user 表，schema 1 拒绝后文件
  字节和用户记录不变；文件恢复保留 partial success 且不重复未知文件动作。
- 廉价非递归分页、过滤交集/去重、SQLite 阻塞时准入锁外 I/O、selection 顺序、
  cancellation/generation/stale publication、独立 WAL reader 和真实 HEIF 回调/热产物。
- 等待 ExifTool install gate 的任务停止后不执行下载；真实 worker 未退出不假称 joined。

## 正式 WebView 媒体验收

使用仓库包装器 `pnpm tauri build --no-bundle`，内嵌生产前端；固定 FFmpeg 9.0.1。
原程序副本与新程序使用相同的 FFmpeg/ffprobe 二进制、夹具和原预算。每次测试使用
runner-owned 隔离状态并在退出后清理，不打开用户数据库。保留了用户原来的 D 盘实例。

12 个场景共 34 次测量，两套程序均通过。包括 HIF 冷/热/完整图、1100 路径缩略图
保留、80 路径胶片条，以及真实 RAW/JPEG 冷/热/完整图与目录浏览。HIF 为仓库真实
DSC00449.HIF；RAW 从用户指定的 30 张 ARW 中按序列分布复制 16 张，原片仅读取。
JPEG 为这些 RAW 的原生相机内嵌 JPEG，经已有生产路径导出，保留 EXIF 方向；这不是
额外的独立 JPEG 相机/格式矩阵。OS 文件缓存未强制清空，cold 指应用状态/产物缓存。

下表为同机中位数，单位 ms；负值表示本次样本新程序更短。每普通场景 3 次，1100
路径保留场景 1 次。小样本不证明统计显著提速；不同照片/NAS/平台不能从此推断。

| 场景 | 指标 | 原结构 | core | 差异 |
| --- | --- | ---: | ---: | ---: |
| cold-preview-hif | firstPageMs | 88.6 | 102.8 | +16.0% |
| cold-preview-hif | firstPreviewMs | 111.6 | 105.3 | -5.6% |
| folder-thumbnail-retention-hif | firstPageMs | 73.7 | 61.5 | -16.6% |
| filmstrip-scroll-hif | firstPageMs | 76.3 | 74.3 | -2.6% |
| warm-loupe-hif-artifact | firstPageMs | 90.2 | 104.6 | +16.0% |
| warm-loupe-hif-artifact | firstPreviewMs | 109.0 | 105.4 | -3.3% |
| warm-loupe-hif-artifact | fullMs | 163.1 | 164.6 | +0.9% |
| loupe-full-hif | firstPageMs | 94.4 | 103.0 | +9.1% |
| loupe-full-hif | firstPreviewMs | 119.7 | 105.2 | -12.1% |
| loupe-full-hif | heifAllTilesMs | 855.0 | 804.5 | -5.9% |
| cold-preview-arw | firstPageMs | 89.7 | 104.3 | +16.3% |
| cold-preview-arw | firstPreviewMs | 282.5 | 273.3 | -3.3% |
| cold-preview-arw | fullMs | 290.9 | 287.1 | -1.3% |
| warm-loupe-arw | firstPageMs | 90.2 | 105.9 | +17.4% |
| warm-loupe-arw | firstPreviewMs | 147.8 | 149.5 | +1.2% |
| warm-loupe-arw | fullMs | 280.6 | 281.7 | +0.4% |
| loupe-full-arw | firstPageMs | 89.5 | 87.3 | -2.5% |
| loupe-full-arw | firstPreviewMs | 148.5 | 154.7 | +4.2% |
| loupe-full-arw | fullMs | 281.6 | 288.2 | +2.3% |
| warm-loupe-jpeg | firstPageMs | 103.6 | 100.5 | -3.0% |
| warm-loupe-jpeg | firstPreviewMs | 121.8 | 125.2 | +2.8% |
| real-raw-directory | firstPageMs | 72.2 | 71.3 | -1.2% |
| real-jpeg-directory | firstPageMs | 71.4 | 72.0 | +0.8% |
| cold-preview-jpeg | firstPageMs | 105.6 | 105.1 | -0.5% |
| cold-preview-jpeg | firstPreviewMs | 274.0 | 277.5 | +1.3% |
| cold-preview-jpeg | fullMs | 263.2 | 277.5 | +5.4% |

主要预览/完整图指标差异约 -12% 到 +5.4%，没有超过 20% 回归阈值。部分首屏增加
约 15–16 ms，仍低于 300 ms。RAW 热首预览中位数 149.5 ms，接近 150 ms 上限；
单次先导样本 150.2 ms 曾超出，正式三次组通过，不能据此声称有充足余量或 p95 达标。

验收修复了工具的过时假设：真实路径是 core-v2/previews/media-cache-v3；HIF 完成
同时接受缓存图和可见、无失败的原生全帧；RAW 可以从缩略图直接交付完整相机 JPEG，
不强制等待不存在的中间 preview。热 RAW 明确检查 full-level cached artifact。
保持 800/150 ms 预算，没有以未完成 warmup、缩略图或失败瓦片冒充完整交付。

## 人物后台分析

使用已安装并校验的 SCRFD/AdaFace、ORT DirectML，16 张相同 JPEG 副本、临时数据库，
release 运行原/新结构各三轮 serial + pipeline。12 轮逐图脸数完全一致：15 张单脸、
1 张双脸，共 17；每轮持久 32 个阶段，没有失败任务。模型目录仅读取。

| 模式 | 原结构中位数 ms | core 中位数 ms | 差异 |
| --- | ---: | ---: | ---: |
| serial | 4184.0 | 4149.8 | -0.8% |
| pipeline | 2860.3 | 2773.7 | -3.0% |

流水线吞吐没有超过 20% 回归；这个对照证明输入/执行/持久一致性与该样本吞吐，不能
替代召回率、跨场次阈值、人工审阅负担或模型准确率评估。跨平台/GPU 矩阵尚未验证。

## 增量构建成本

分别 warm prime 独立 target，再临时修改人物 serde 类型、标签名字规则、媒体 resize
算法，计时 workspace cargo check/build；每步恢复原始字节及哈希并重新 prime。原/新
worktree 的临时源码均已恢复。测量不运行临时行为，也不修改用户原 checkout。

| 单次编辑 | 原 check 秒 | core check 秒 | 原 build 秒 | core build 秒 |
| --- | ---: | ---: | ---: | ---: |
| person-type | 7.562 | 5.258 | 31.685 | 25.348 |
| tag-rule | 4.657 | 4.957 | 20.810 | 20.847 |
| media-algorithm | 5.224 | 6.133 | 22.638 | 24.318 |

人物类型的 check/build 本次分别短约 30.5%/20.0%；标签规则 check 长约 6.4%，build
基本持平；媒体算法 check/build 长约 17.4%/7.4%。每类只测一次真实编辑，不是统计
显著性结论。合并没有让所有改动更快：本次人物契约不再向 media 传播，媒体改动仍需
重编译/链接上层，core 编译单元较大。保留 media/parser 的独立边界有明确价值。

## 检查与可复现范围

恢复测量中的临时源码后，完整 contracts-feature workspace：786 Rust 测试通过、
29 ignored；格式、严格 all-targets Clippy 和生成契约漂移检查通过。前端类型检查、
71 个文件的 415 项测试和生产构建通过；包含两个完整交付门槛回归。正式 Tauri
release 构建通过。Ignored GPU/native 场景另有上述显式执行证据。

本轮是 Windows 本机验收。Linux/macOS 的 CI 配置已随目标组织更新，但未在本机
执行，不宣称跨平台已验收。没有更新 pinned native revision 或第三方 notices。

详细原始报告和配置位于本机 `target/core-refactor-qualification`（git-ignored），包括
final-reports、baseline-reports-expanded、native-performance-comparison.json、
directml-comparison.json、夹具 manifest、构建与检查日志；私人照片不纳入 Git。
完成记录见 [core-layout-refactor](../archive/plans/core-layout-refactor.md)。
