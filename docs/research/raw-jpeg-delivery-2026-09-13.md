# CR3/RW2 大 JPEG 选择与 Windows 图像交付

## 修复

CR3 的部分 JPEG track 在 LibRaw 中只有字节范围，宽高为零。旧选择器选中 1620×1080，
漏掉实际 6000×4000 的 JPEG。现在只对未知尺寸的 JPEG 范围解析 SOF，再按既有最大面积
或最小足够尺寸规则选择，不先解码像素。

RW2 除 LibRaw 已知的 IFD0 `0x002e`，额外解析 `0x0127` 的 `JpgFromRaw2`。Panasonic
DC-S5M2X 样本中，前者为 1920×1280，后者为 6000×4000。后者的 TIFF 字节数包含 62 字节
对齐填充；只去掉 EOI 后的 00/FF，保留严格缓存完整性检查。先前把 LibRaw 列表等同于全部
预览，因此“该 RW2 没有完整 JPEG”的结论已修正。标签依据：
[ExifTool PanasonicRaw](https://exiftool.org/TagNames/PanasonicRaw.html)。

Windows/macOS/Linux 共用选择逻辑；保留 LibRaw EXIF 方向处理和 JPEG 原有颜色信息。
相机 JPEG 不执行像素解码、锐化或有损重编码，也不声明传感器 native detail。full/preview
投影策略与 largest-JPEG 精确产物身份已更新，避免旧选择抑制升级。Interim/Satisfied、lease、
generation 和异步持久化边界不变。没有修改 vendored LibRaw。

读取有界：JPEG 头最多 1 MiB / 4096 markers，缓冲 4 KiB，最多 8 个 LibRaw 候选。
RW2 只读一个 IFD0，最多 1024 entries；标签必须是 UNDEFINED 字节数组，范围不越界且
不超过 128 MiB，尾部检查最多 4 KiB。生产路径没有全文件 SOI 扫描；无效/超预算候选
不排挤已知预览，没有足够 JPEG 时继续既有显影链。

## 实测

Windows Release。Canon EOS R6 Mark II 的 `HI8A7823.CR3` 来自含 942 张 CR3 的 NAS
目录；Panasonic DC-S5M2X 的 `PANA4954.RW2` 使用本地真实夹具。照片、私有路径和截图
只存于被忽略的诊断目录。应用数据/缓存隔离；未清理 OS/NAS 缓存。

| 路径 | 修复前 | 修复后 |
|---|---|---|
| CR3 普通 full | WIC 显影 4024×6022 | 相机 JPEG 4000×6000 / Satisfied |
| RW2 普通 full | 小 JPEG 不足，转显影 | JpgFromRaw2 4000×6000 / Satisfied |
| CR3 原生 full 管线 | 4325 ms（旧单次） | 138 ms（新单次） |
| RW2 原生 full 管线 | 本轮显影回退 6531 ms（单次） | 65 ms（新单次） |
| CR3 选中→完整图加载 | Release 4743 ms（旧单次） | 257 ms 中位，274 ms 三次最大 |
| RW2 选中→完整图加载 | 历史构建不与本次混算 | 354 ms 中位，361 ms 三次最大 |

WebView 每样本三次，冷应用缓存；不是 942 张不同照片的完整基准。CR3 目录首屏中位
483 ms，仍未达到 300 ms 预算。`image:loaded` 沿用解码就绪/状态更新口径，不等于精确
GPU present；方向与最终画面另做截图检查。

| full 交付阶段 | CR3 相机 JPEG | RW2 相机 JPEG |
|---|---:|---:|
| 编码字节数 | 1,738,566 | 6,056,236 |
| Resource Timing duration，中位 | 18.3 ms | 44.7 ms |
| 原生 materialize，中位（包含在上一行） | 1.55 ms | 1.43 ms |
| load 后等待 img.decode，中位 | 86.1 ms | 140.0 ms |

## 约 0.5 秒由什么组成

在实际 Release 窗口等待 942 张缩略图全部保留后，点击“重新显影当前图片”。同一 CR3
输出 15,988,018 字节的 4024×6022 WIC JPEG。两次 load 后的 decode 等待约 312 / 309 ms。
第二次的完整分段：

| 阶段 | 耗时 |
|---|---:|
| 新 full URL 提交→资源完成 | 138.4 ms |
| 其中 native worker dispatch | 0.017 ms |
| 其中 native materialize（读取/复制/校验） | 10.555 ms |
| 资源完成→img load | 47.4 ms |
| load→img.decode 完成 | 309.4 ms |
| URL 提交→完整图加载标记 | 495.5 ms |

这不是 495 ms 的 JSON IPC RTT，也不能将全部 138 ms 称为网络传输。Windows 本机
`oxy-media` 协议没有实际 HTTP 服务器。registry 将文件读为 Vec 或复制 Arc 编码字节；
Wry 0.55.1 的 `prepare_web_request_response` 通过 `SHCreateMemStream` 和
`CreateWebResourceResponse` 交付到 WebView2。后面还有 WebView 进程交付与调度；
本次没有把每一次内部复制分别精确计时。

原 WIC 后端三次测量中位：解码 1475 ms、锐化 835 ms、JPEG 编码/写盘 1477 ms，
完整输出 3786 ms。锐化在约 2400 万像素上构造模糊与输出缓冲，编码也是全像素工作。
64 KiB 写缓冲已经存在；继续调写缓冲不能消除这些 CPU 工作。

**同日策略纠正：上述是移除 RAW 额外锐化前的历史数据。** 锐化只对已验证的 Sony HIF
保留；WIC / LibRaw 生产路径与 RAW benchmark 已删除该后处理。上述 16 MB 与 496 ms
也属于旧锐化输出，不能作为移除后的最新测量；不会以简单相减代替复测。

移除后对同一 NAS CR3 强制 WIC 实测一次：原生完整管线 3137 ms，其中 WIC 处理与
JPEG 发布 3070 ms；产物 4024×6022 / Satisfied，处理记录只有 develop、colorConvert、
orient、encode。该次未重测 WebView 交付延迟。新增回归验证输出与直接编码解码器像素
一致；设置明确标注 Sony HIF 锐化，非 SHIF brand 不启用。历史产物使用现有清空缓存入口，
不增加针对旧处理问题的特殊兼容分支。

## Passthrough 与后续优化

后续共享像素与原生渲染面的工作已整理为[远期项目计划](../tasks/native-image-presentation-plan.md)，
并列入[路线图](../ROADMAP.md#long-term-native-image-presentation)。原型与架构选择均未启动。

1. **相机 JPEG 直通：已实现并验证。** 编码字节经 `ProducedPayload::Encoded` 发布给 img，
   不等后台持久化。IPC 只传描述符，图像不进 JSON/base64。浏览器负责最终 JPEG 解码。
   CR3 从约 16 MB 显影 JPEG 换成约 1.7 MB 相机 JPEG；处理和传输改善均已实测。
2. **更快的显影 JPEG：可独立优化，尚未改。** 可评估已有 libjpeg-turbo 的 SIMD 编码，
   保持 quality、采样、ICC、方向和取消语义；不附加未经官方解码对比验证的后处理。
   这些不能消除 WebView 再解码 JPEG；简单降低 quality 不算同质量优化。
3. **共享 WIC 像素→Canvas/WebGL：可行方向，尚未实现或测得收益。** WebView2
   [PostSharedBufferToScript](https://learn.microsoft.com/en-us/microsoft-edge/webview2/reference/win32/icorewebview2_17)
   将文件映射共享为 JS ArrayBuffer，可用于跳过 JPEG 编码、协议整块复制和 JPEG 再解码。
   它不是可直接显示的纹理；约 72.7 MB RGB / 96.9 MB RGBA 仍涉及颜色/方向、纹理上传和
   绘制，以及 buffer/lease/cancellation 生命周期，不能称为零拷贝上屏。
4. **原生 D3D 纹理/独立画布：更彻底的方向，尚未实现。** 让照片留在原生渲染面，WebView
   管控件与变换信号，可避开照片经 WebView 协议交付。需要处理画布合成、缩放拖动、裁剪、
   DPI、叠加控件、ICC/HDR、资源回收及其他平台实现。应先用同一 WIC 像素做最小原型，
   对比分阶段延迟和峰值内存，再确定正式架构。

本次没有通过取消 `img.decode` 等待来提前露出尚未解码的空图，也没有降低图片清晰度。

## 验证

有界头读取、候选范围溢出/截断、RW2 标签类型/数量/范围、EOI 填充有 focused tests。
显式配置真实 CR3/RW2 fixture，覆盖 4096/full 选大 JPEG、full 的 Embedded / Satisfied /
4000×6000 和无 develop/sharpen/encode。前端类型检查、216 项测试与生产构建通过；Rust
workspace tests / doctests、含 bench-tools 的 Clippy 和格式检查通过。实际 Release WebView
确认两种格式的完整图片与方向正确，测试窗口在结束后关闭。

新增 Server-Timing、Resource Timing、load/decode 标记仅供显式性能场景；常规使用没有
额外图像请求。仅实测 Windows，尚未全面验证 macOS/Linux 和其他相机型号。
