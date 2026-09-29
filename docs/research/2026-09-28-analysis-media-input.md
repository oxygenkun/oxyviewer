# 识别与媒体输入接口验证（2026-09-28）

## 接口与职责

`oxy-people` 声明 `oxy-domain::AnalysisRequirement`，当前原图最低长边 112、
目标长边 1600。worker 将需求传给输入回调；Tauri 仅装配目录与媒体服务。
`oxy-media::AnalysisInputService::prepare` 选择表示并返回 RGB8、源版本、
`ArtifactFacts`、颜色状态、ICC 存在性、缓存命中。方向在媒体侧应用一次；
BGR、模型归一化、检测、五点对齐及特征提取归识别侧。

表示必须覆盖完整参考画面，并有足够的真实采样；不放大原图，不使用显示锐化。
RAW/HEIF 优先复用经 manifest、源版本、采样、lease 与 generation 校验的媒体缓存。
无合格缓存时：JPEG 缩放 IDCT；RAW 最大相机 JPEG，不足才 LibRaw 缩减显影；
HEIF 复用已有平台后端 planner 并对每个候选附加资格判断；PNG/WebP/TIFF 使用
有界 raster 解码，透明像素合成到白底。输入服务不生成第二份分析缓存。

保持独立单槽 512 MiB 临时预算、输入前后源版本校验、取消与结果提交代际检查。
FFmpeg 辅助预览现在使用既有可取消进程执行器，终止后回收子进程。
不可中断的原生调用返回后检查取消；源探测、系统文件 I/O 并非硬实时可中断。
模型 session、WebView 和保留的下采样帧不属于临时解码预算。

输入 schema 升为 `media-oriented-rgb-full-frame-1600-v2`，pipeline version 为 2，
特征空间为 `adaface-webface12m-f2eb07d0-media-v2`。模型权重不变；旧版本空间和
人工记录保留，新运行写入独立空间。回归测试先存入旧空间特征，再写新空间并分别查询，
防止升级后因 `producer_fingerprint` 冲突而全部编码失败。

## 真实格式 Release 测量

Windows x64、本地副本、Release Rust 测试，目标 1600；没有清空 OS 文件缓存。
首次／重复列均没有合格展示 artifact；“展示缓存”列先调用真正的媒体展示产物生产器，
再由分析服务查询复用，并断言 `cache_hit=true`。缓存生产耗时不计入命中耗时。
以下仅为输入准备时间，不包含 ONNX session 加载、检测、特征编码或 UI。

| 样本 | 首次 | 重复 | 展示缓存 | 选择 |
| --- | ---: | ---: | ---: | --- |
| DSC00449.HIF | 2781 ms | 2693 ms | 247 ms | libheif 主图；热路径读取完整 JPEG artifact |
| DSC00835.jpg，6336×9504 | 306 ms | 289 ms | 不查展示缓存 | libjpeg-turbo 缩放 IDCT |
| DSC01332.ARW | 169 ms | 167 ms | 168 ms | 相机 JPEG，应用 EXIF 8 |
| HI8A7823.CR3 | 206 ms | 199 ms | 201 ms | 相机 JPEG，应用 EXIF 8 |
| PANA4999.RW2 | 114 ms | 113 ms | 116 ms | 1920×1280 相机 JPEG，应用 EXIF 8 |

所有返回图为 1067×1600、无上采样，事实与源版本一致。
这组 RAW 在本地已经有便宜的相机 JPEG；缓存命中没有可声称的显著速度收益。
HIF 冷路径仍需主图解码，不能把它表述为已达到浏览界面的 800 ms 冷预览目标。
该界面预算与后台输入时间也不是同一测量口径。

夹具验证发现并修复两处选择问题：

- CR3 相机 JPEG 6000×4000 与传感器有效区 6022×4024 有边缘差异。
  严格按原图比例推导短边会不必要地触发显影。现只对推导短边放宽 1%，
  目标长边仍严格；此样本从约 878–903 ms 的显影回到相机 JPEG。
- Sony HIF 的 FFmpeg 辅助流为 1664×1088，缩到长边 1600 后短边只有约 1046，
  不满足完整画面的要求。候选被拒绝后继续回退，而非返回低质量输入或直接中止任务。
  160×120 内嵌 JPEG 也不会被采用。

复现（从仓库根目录，以 PowerShell 设置 `OXY_ANALYSIS_FIXTURES` 到自备夹具目录）：

```powershell
$env:OXY_FFMPEG_DIR = (Resolve-Path target/release).Path
cargo test -p oxy-media --release analysis_input --lib -- --include-ignored --nocapture
```

本机原始产物在忽略目录 `tests/perf/.reports/analysis-input-20260928/`，
`input-tests.log` 保留全部来源、尺寸、耗时与实际缓存命中。

## 验证与边界

Windows Release/WebView2 通过 `agent-browser` 操作隔离资料库，实际点击识别按钮：
5 张 JPEG/HIF/ARW/CR3/RW2，10/10 步成功、0 失败，产生 7 组 512 维特征，
每组 2048 字节（RW2 检测到 3 张脸，其余各 1）。目视核对 HIF 与 EXIF 8 的 CR3
人脸框均覆盖正确位置。中途取消另一次运行后，账本停在 5/10，状态 cancelled，
已有 7 组特征保留。

最终构建重启并对已有缓存资料库重新分析，仍是 10/10、0 失败；
`adaface-webface12m-f2eb07d0-media-v2` 新空间有 7 组，前一空间的 7 组仍保留。
完整旧 v1 manifest 的升级兼容由上述领域回归测试单独覆盖。
最终运行账本为 38 秒（加载模型后起算、秒级精度，不是精确按钮延迟）；
主进程峰值工作集 882,110,464 字节，含模型与原生图像任务，不含 WebView／FFmpeg
子进程。这里没有把 512 MiB 临时输入预算误当作进程峰值上限。
测试实例与独立浏览器会话在验证后关闭。

证据：`hif-detection.png`、`cr3-detection.png`、`final-completed.png`、
`native/final-state.json` 和 `start-native.ps1` 位于上述忽略目录。

- Release 输入测试 8 项通过（含显式执行的真实夹具测试）：源变化、取消、方向、
  两轴细节、相机边缘差异、PNG/WebP/TIFF 通道／透明度、低清和锐化缓存拒绝、实际缓存复用。
- 前端类型检查、构建及 67 个文件／372 项测试通过。
- 人物域 48 项、共享域 11 项、桌面 Rust 44 项通过，手动环境测试分别忽略 2／0／1 项；
  `cargo fmt --all --check` 和最终 Release 构建通过。
- 全工作区测试在媒体库安全基线处失败：本机 libheif 1.23.3，要求至少 1.23.4；
  同一轮媒体测试 207 通过、13 忽略，仅此项失败。没有修改固定 native 依赖或绕过检查。
- 严格工作区 Clippy 被旧代码的 `collapsible_if`／`manual_is_multiple_of` 阻断；
  定向 `--no-deps` 检查在命令行允许这两类旧告警，无源码 lint 豁免。

颜色保留现有解码语义：相机编码 RGB／未知色彩标记为 `EmbeddedOrUnknown`，
LibRaw 显影标记 sRGB；不是所有格式已统一 ICC/HDR 的承诺。尚未验证 macOS/Linux
实机、全部相机与 TIFF 编码变体、NAS、大文件夹并发浏览退化、跨色域识别质量。
模型仍只取得一张最多 1600 长边的整帧；小脸高分辨率 ROI 与在途展示任务合并尚未实现。

## HIF 慢路径复查：主图 FFmpeg 缩放实验

同日针对“浏览能在 1 秒内读取，识别为什么需要 2.7 秒”复查。当前识别使用
Preview 计划：FFmpeg 辅助流不合格后直接进入 libheif，没有使用浏览已经具备的
FFmpeg 主图 tile-grid 路径。libheif 主图先解码为全尺寸 RGB 再缩放；本机 Windows
libheif 使用 libde265，而浏览使用固定的 FFmpeg。因此此前耗时不是 HIF 输入的下限。

使用同一 DSC00449.HIF、`target/release/oxy-ffmpeg.exe`，顺序运行各 3 次：

| 子过程 | 三次耗时（ms） | 输出 |
| --- | --- | --- |
| ffprobe 容器与流信息 | 47 / 42 / 48 | JSON |
| 当前辅助流路径 | 174 / 193 / 178 | 1046×1600 JPEG，最终被资格检查拒绝 |
| FFmpeg 主图拼接、裁边、转向、缩放 | 733 / 679 / 654 | 1067×1600 RGB BMP，5,126,454 字节 |
| 同一主图路径，改用 JPEG 输出 | 641 / 615 / 626 | 1067×1600，209,402 字节 |

主图实验使用完整六块主图，无小预览替代，无锐化。过滤器如下，输入解码线程和
filter_complex 线程均指定为 2；最终 BMP 使用 `-c:v bmp -f image2pipe pipe:1`。

```text
[0:0][0:1][0:2][0:3][0:4][0:5]xstack=inputs=6:layout=0_0|3520_0|0_1600|3520_1600|0_3200|3520_3200,crop=7008:4672,transpose=clock,scale=1067:1600:flags=area,format=rgb24[out]
```

这里测量包含进程启动和 stdout 回收，不含 ffprobe、Rust 位图加载、模型推理及排队；
未清理操作系统文件缓存，也未验证 NAS 或并发浏览。尺寸与布局针对该夹具，生产代码
必须继续从现有 grid 解析结果生成，不能硬编码。原始时间与图像位于忽略目录
`tests/perf/.reports/analysis-input-20260928/hif-optimization/`。

建议优先顺序：在解码前按两轴和覆盖条件排除不合格辅助流，节省约 0.18 秒；
增加 FFmpeg 主图直接缩放为有界 RGB 的候选，放在 libheif 前；保留现有合格缓存优先。
RGB/BMP 可避免有损 JPEG 中转，实验表明仍有达到 1 秒以内的空间。现阶段仅完成
路径诊断与候选性能实验，当时尚未接入生产，也未完成跨解码器色彩／特征一致性、取消、
峰值内存和前台并发回归验证。不能将子过程实验写成已实现的端到端延迟保证。

## 共用选择器落地

随后将 HEIF 选择策略收敛为以下三层，而不是增加人物专用的快速解码分支：

- `FrameRequest`：目标长边、细节要求、是否允许临时画面；资格检查复用缓存尺寸规则。
- `heif::planner`：同一平台顺序服务像素、JPEG artifact 和 loupe tile session；
  操作类型只保留交付能力差异。Windows/Linux 优先 FFmpeg，macOS 优先 ImageIO。
- `heif::artifact::decode_frame`：预览与分析共用。FFmpeg/libheif 先检查辅助表示，
  不符合请求则读取主图。FFmpeg 共用浏览的 grid、裁边和方向逻辑，在子进程内缩放，
  以无损 BGRA BMP 交付，复用已有 BMP 校验/读取器，不增加原生依赖。

首次 Release 集成测量：同一 HIF 无展示 artifact 时为 **890 / 797 ms**，输出
1067×1600，事实来源 PrimaryImage/FFmpeg；热展示 artifact 为 250 ms。
这些时间包含 `AnalysisInputService::prepare` 的探测、选择、位图读取和 RGB 准备。
另一轮同时执行真实像素一致性测试时出现 1577 ms，说明并发压力仍会影响时间；
不能把单任务结果当作并发延迟保证。未清空系统文件缓存。

真实回归验证：相同请求经共享画面入口和分析服务得到逐像素相同的输出，且没有 JPEG
中转编码；辅助流两轴不足在解码前被拒绝，NativeDetail 不会选择相机辅助流；
Windows/macOS/Linux 的预览与 session 平台顺序由纯策略测试约束。
Release loupe 原生基准的冷 session 为 FFmpeg 六块、966 ms（解码 965 ms）；
后两次为缓存命中，不与冷路径混算。该基准测原生发布，不是 WebView 实际绘制。

输入 schema、pipeline 和特征空间升级为 v3，使旧派生特征与新选择器输出隔离；
不改变模型权重或人工标注。缓存、源版本、取消和各自队列边界保留；没有合并在途任务。
本次没有重做桌面 UI 操作、跨解码器色彩/识别精度校准、NAS 或 macOS/Linux 实机验证。

最终串行 Release 输入复测：HIF 794 / 678 ms，展示缓存 263 ms；9 项输入测试全部通过。
`cargo test --workspace -- --test-threads=1`：735 通过、23 忽略，含媒体 210 项。
默认并行工作区测试在 4 项真实 FFmpeg 测试中失败，错误为内存／编码线程分配失败，
所以真实 native 夹具按串行验证。`cargo fmt --all --check` 通过；严格定向 Clippy
仍被旧 `collapsible_if`／`manual_is_multiple_of` 告警阻断，命令行仅放行这两类后
`oxy-media`／`oxy-people --all-targets --no-deps` 通过，未加入源码豁免。
原始日志在 `.test-tmp/shared-frame-{inputs-serial,loupe,workspace-serial,clippy-focused}.log`。

## 全格式共用媒体准备（v4）

本轮将上一阶段的 HEIF 共用入口扩展为 media 层职责边界：
`MediaRequest` 共用缓存与候选的细节／呈现资格，`MediaService` 管源版本、缓存、
lease/generation 和像素准备，dispatcher 统一路由到各格式模块。
`AnalysisInputService` 不再有格式分支、缓存查找或解码器，仅做模型需求适配与 RGB8
白底合成。JPEG 共用候选探测、资格／排序、ICC／方向和缩放 IDCT；RAW 共用相机
表示、来源事实和显影后端顺序；HEIF 保留同一 planner／frame selector；
PNG／WebP／TIFF 共用定向像素与 ICC/alpha 保留，TIFF 展示用 PNG 发布。
直接编码原图与渐进瓦片仍保留，不要求所有 UI 绕行像素 API。

同缓存目录、源版本、策略、generation 和呈现语义相容的展示生产结果可通过资源
lease 交给像素消费者，不等待持久化。仅等待可证明满足细节的生产请求，累计最多
50 ms，不占解码槽；消费者取消不会取消生产者。超时或不相容自行按共用流程准备。
这不是双向全局 single-flight，也没有把运行中的瓦片拼成模型输入。

模型输入 schema、pipeline version、feature space 升为 v4；模型权重不变，旧派生
特征隔离，人工数据保留。macOS Core Image 的编码输出能力仍与 ImageIO/LibRaw
像素输出不同，像素请求按同一后端顺序跳过不能提供像素的后端；尚无 macOS 实机证据。

### Windows Release 实测

相同五张夹具、串行执行、未清操作系统文件缓存。下表计输入准备，不含推理。

| 样本 | 未命中展示 artifact 两次 | 合格展示 artifact |
| --- | --- | --- |
| DSC00449.HIF | 761 / 703 ms | 307 ms |
| DSC00835.jpg | 349 / 351 ms | 原始样本优先 |
| DSC01332.ARW | 209 / 204 ms | 188 ms |
| HI8A7823.CR3 | 253 / 246 ms | 234 ms |
| PANA4999.RW2 | 153 / 145 ms | 138 ms |

均为 1067×1600；HIF 为 FFmpeg PrimaryImage，RAW 为相机 EmbeddedPreview，
没有以不足两轴的辅助图充数。HIF 相对最初 libheif 2693–2781 ms 的路径明显缩短；
与前一阶段 678–794 ms 大致同一区间，其他格式受本机负载影响，不宣称本轮普遍提速。

`heif_display_bench … 3 all` 首轮全图为 FFmpeg 六瓦片，decode 634 ms、publish
1 ms、total 635 ms；后两轮为缓存，total 325 / 332 ms。后两轮不是冷解码，
不合并成冷路径中位数。该基准只证明原生交付，不代表 WebView 首次绘制时间。

### 回归边界

- Release 媒体库：214 通过、14 忽略。
- 显式启用真实输入夹具：11 通过，包含 HIF 公共入口与模型输入逐像素比较、JPEG
  EXIF 方向、PNG/WebP/TIFF 显示与像素交付的 RGBA 一致性、两轴细节和缓存拒绝。
- 新增 source-level audit 防止人物适配器重新引入格式路由、后端与缓存机制。
- 相容在途结果测试覆盖消费者取消不影响生产者，缓存目录、generation、锐化与
  细节不相容时拒绝复用。未宣称已经测出并发复用命中率或大型文件夹吞吐。
- 前端类型检查、67 文件／372 测试、生产构建通过。首次沙箱内 esbuild 读取父目录
  被系统拒绝，沙箱外重跑通过；没有据此修改应用源码。
- 定向 Clippy 在命令行仅放行既有 `collapsible_if`／`manual_is_multiple_of` 后通过，
  未添加源码豁免。格式及 diff whitespace 检查通过。

原始日志：`.test-tmp/media-unified-{real,loupe,tests-final,clippy3,frontend-check,frontend-test,frontend-build}.log`。
完整工作区与桌面操作结果见下方追加记录。跨色域模型精度、NAS、Mac/Linux 与大规模
并发浏览仍需对应平台和负载实测。

完整工作区串行测试最终为 **739 通过、0 失败、23 忽略**（忽略项中的真实输入已按
上述命令单独启用验证）。日志 `.test-tmp/media-unified-workspace.log`。

### 最终桌面 Release 验证

最终 `cargo build -p oxyviewer --release --features tauri/custom-protocol -j 1`
成功。使用独立 data/cache/WebView 目录启动该二进制，经 agent-browser 操作人物面板
“识别选中文件夹”，五张混合格式照片完成 **10/10 步骤，0 失败**。只读核对数据库：
运行状态 completed，新 `adaface-webface12m-f2eb07d0-media-v4` 空间产生 7 条特征，
旧空间仍隔离保留。没有采用检测框或创建人工身份。

实际检查 HIF、CR3、JPEG Loupe 截图，方向正确、检测虚线框落在人脸上。
截图在 `tests/perf/.reports/analysis-input-20260928/native/unified-{hif,cr3,jpeg}.png`。
CR3 的一次文本等待把 aria-label 当作可见文字，25 秒超时；随后 accessibility
snapshot 和实际截图均确认“采用检测人脸 1”及可见框，属于验证定位器问题。

60 MP JPEG 的原图 Loupe 和识别正常，但胶片栏仍显示占位：现有缩略图转换估算
按原尺寸每像素 6 字节计最坏 progressive 系数，仅此已超过 UI 的 256 MiB 池。
该估算与限制本轮未改；不能把此次记录描述为所有浏览缩略图均已通过。
后台 512 MiB 像素准备不受这个前台 admission 上限影响。此既有大 JPEG 缩略图限制
需要单独依据实际编码类型优化估算，不能为了通过显示而直接提高全局预算。
