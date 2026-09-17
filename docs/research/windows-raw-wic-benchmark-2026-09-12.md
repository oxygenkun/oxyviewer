# Windows RAW full：WIC 与 LibRaw 基准

## 范围与环境

本次先验证系统 WIC 是否值得加入 RAW full 的显影回退链。基准直接调用后端，绕过
内嵌 JPEG 和 OxyViewer artifact cache；不把相机 JPEG 提取耗时与 RAW 显影混为一谈。

- Windows 11 Pro，build 26200.9278，x64。
- Intel Core i7-13700K，16 核 / 24 逻辑处理器。
- OxyViewer 固定版本的 LibRaw 0.22.2 source submodule，release 构建。
- Microsoft Raw Image Extension 2.5.35.0，Microsoft Raw Image Decoder，
  CLSID `41945702-8302-44A6-9445-AC98E8AFA086`。
- 两张本地 Sony ARW。原始照片、机器绝对路径和输出图像不进入 Git。
- 每次启动独立进程；两个后端串行执行，轮流先运行。未清理 OS 文件缓存，属于
  **新进程、无应用缓存、OS 文件缓存可能已热**的测量，不是冷盘或 NAS 基准。

| 样本 | 字节数 | SHA-256 | 输出 |
| --- | ---: | --- | --- |
| DSC01332.ARW | 67,973,120 | AE0B85849AF02C64E2D9F0C7AA586AE482ED6D5FE5F5A8FE27F16AA0D33575BA | 4688×7028，EXIF orientation 8 |
| DSC08653.ARW | 43,151,360 | BD1716E06382DEDA29C4C8F9A3A395A6721633B94C7F62F327B2D4901FFBCC51 | 4688×7028，EXIF orientation 6 |

## 解码结果

每个样本、后端 5 次。decode 包含文件打开、像素解码、复制和方向应用，不包含后续锐化、
JPEG 编码、落盘、IPC 或 WebView 图片加载。WIC 使用 `GetFrame(0)` 原生 RGB24，
LibRaw 使用当前 full 参数（full size、camera white balance、sRGB8、AHD）。

| 样本 | WIC 中位数 | LibRaw 中位数 | 解码速度比 |
| --- | ---: | ---: | ---: |
| DSC01332.ARW | 1633.60 ms | 5627.41 ms | 3.44× |
| DSC08653.ARW | 2555.83 ms | 6442.09 ms | 2.52× |

这证明当前机器和两张 Sony RAW 上的 WIC 解码更快，不代表所有相机、压缩方式或 codec
版本。两个解码器使用不同的显影策略，不能把同尺寸解释为像素完全一致或画质完全等价。
没有 GPU 使用证据，不将 WIC 称为硬件加速。

五次运行中最大的 sampled peak working set：

| 样本 | WIC | LibRaw |
| --- | ---: | ---: |
| DSC01332.ARW | 498.57 MiB | 512.42 MiB |
| DSC08653.ARW | 484.69 MiB | 519.52 MiB |

原始数据：[decode.json](windows-raw-wic-benchmark-2026-09-12/decode.json)。

## 能力与正确性发现

1. 真实用户环境已安装扩展，但受限执行环境看不到它：WIC 打开 RAW 返回
   `WINCODEC_ERR_COMPONENTNOTFOUND (0x88982F50)`。实际安装用户环境中的同一个程序能够
   打开文件。安装包查询和 WIC 检测都必须在应用实际运行的用户/权限环境进行。
2. 微软这个版本的 decoder 不暴露 `IWICDevelopRaw`，查询返回 `E_NOINTERFACE
   (0x80004002)`。因此不能将该接口作为所有 WIC RAW 后端的唯一入口，也不能声称已为
   微软 decoder 设置 `BestQuality`。基准保留严格 `wic` 模式来重现此差异，`wic-frame`
   测量其实际可用的完整帧入口。
3. `GetColorContexts` 返回 unsupported，但帧 EXIF ColorSpace 为 1。生产接入仍需明确
   验证颜色与方向契约，不能把转换为 RGBA 与转换为 sRGB 混淆。
4. 输出 7028×4688 存储像素，应用 EXIF 方向后为 4688×7028。样本一的最大内嵌 JPEG
   为 7008×4672，不能把它与此次完整帧输出混为一谈。
5. 已使用 Windows WPF 的独立 WIC 解码检查样本一，并检查磁盘输出的缩小预览。
   早期大图查看工具显示了花屏，但独立读取同一个磁盘文件证明文件完整，不能将该
   显示工具的问题记录为 WIC decoder 或 RGBA 转换器缺陷。两张样本的方向、完整构图和
   可见颜色均进行了缩小预览检查；这不是色彩校准或所有相机的画质认证。
6. 对第二张样本同一位置的 700×500 原生像素裁剪进行比较，WIC 的彩色噪点更明显，
   LibRaw 的肤色和暗部更平滑。这与压缩后 WIC JPEG 更大相一致，但不能仅凭 JPEG
   体积推断画质。默认接入系统解码时应保留用户选择内置显影的入口。

## 完整 JPEG 输出

另测两种后端相同的 `unsharpen(0.8, 2)`、JPEG quality 95、原子落盘流程，分开记录
decode、sharpen、encode/write 和 total。当前 writer 直接将大量小写入发送给文件；
完整输出基准与 64 KiB 写缓冲实验分开记录，避免把编码/写盘优化归功于 WIC。

### 当前无缓冲 writer

| 样本 | 每后端完成次数 | WIC 总耗时 | LibRaw 总耗时 |
| --- | ---: | ---: | ---: |
| DSC01332.ARW | 3（中位数） | 50.31 s | 51.69 s |
| DSC08653.ARW | 1（单次） | 179.16 s | 144.04 s |

第二张图仅 WIC encode/write 已花费 175.60 s，LibRaw 为 136.85 s。完整的一对结果
写入报告后，主动终止第二次 LibRaw 重复，转做写缓冲实验；该被终止的运行不计入结果，
不能把第二张图描述为三次中位数。WIC JPEG 为 23,146,755 字节，LibRaw 为 18,019,004
字节，两种显影结果的压缩量不同，使无缓冲小写入开销的差异进一步放大。

因此 **只替换解码器，并不能保证当前流程的最终文件更快生成**。
原始数据：[jpeg.json](windows-raw-wic-benchmark-2026-09-12/jpeg.json)。

### 相同 64 KiB 写缓冲

此实验保持 JPEG 编码器、quality 95、锐化参数、`sync_all` 与原子发布，给写入增加
64 KiB `BufWriter`；没有把缓存同步移出计时。每样本、每后端 3 次。

| 样本 | WIC 总耗时中位数 | LibRaw 总耗时中位数 | 速度比 |
| --- | ---: | ---: | ---: |
| DSC01332.ARW | 3.557 s | 7.225 s | 2.03× |
| DSC08653.ARW | 5.060 s | 8.566 s | 1.69× |

JPEG encode/write 降到约 0.96–1.61 s。完整输出时 sampled peak working set 最大约
963 MiB（WIC）和 952 MiB（LibRaw），主要包含显影和公共锐化缓冲；不要将解码阶段
约 500 MiB 的数字称为完整输出峰值。

分别比较两个样本、两个后端的无缓冲/缓冲 JPEG：四组解码后的 RGB 像素 SHA-256
全部一致。**写缓冲没有改变输出像素**；该一致性不表示 WIC 与 LibRaw 输出互相一致。
WIC 探索模式未强行写入 sRGB ICC，LibRaw 延续生产 ICC；小段 ICC 数据对上述写入
瓶颈没有实质影响，生产 WIC 接入必须单独完成颜色契约。

原始数据：[buffered.json](windows-raw-wic-benchmark-2026-09-12/buffered.json)，
[像素等价检查](windows-raw-wic-benchmark-2026-09-12/pixel-equivalence.json)。

## 接入决策

当前结果支持继续接入 WIC，但同时应修复公共 JPEG 写缓冲，才能让用户得到完整输出
的加速。保留最大内嵌 JPEG 优先和 LibRaw 最终回退。微软当前 codec 应走经过尺寸、
方向、颜色和实际像素验证的原生完整帧路径；不能因为没有 `IWICDevelopRaw` 而误报
用户没有安装扩展。其他 codec 是否合格需按其实际能力验证。

基准后已接入生产 WIC 优先、LibRaw 回退、公共写缓冲，以及依赖检测、安装入口、
重新检测和显式重新显影。用户流程与限制见 [Windows RAW](../windows-raw.md)。
上表 total 是后端文件生成耗时，**不是完整应用上屏时间**。

## 工具验证

生产接入初次验收（独立单次，非新增统计基准；后续已移除后端选择功能）：

- sample-2 生产 WIC full 输出 `4688×7028 / RawSensor / native / Satisfied`，
  含 sRGB ICC，耗时约 5.38 秒；系统扩展不可实例化的隔离环境回退 LibRaw，
  同样得到完整图，约 8.98 秒。
- 最终检测同时验证枚举和实例化：真实用户环境识别 Microsoft Raw Image Decoder；
  受限环境只可实例化内置 DNG Decoder，对 ARW 返回 `missing`。没有卸载用户扩展。
- 实际 release WebView 中，sample-1 普通打开走 `4672×7008` 相机 JPEG，选中到
  完整图片加载约 511 ms。点击“重新显影”后从入队到 `4688×7028` 新图加载约
  3818 ms；50 ms DOM 采样期间旧完整图始终可见，folder thumbnail 保留数保持 2。
  native attempt 为 Ready，落盘 facts 确认 `Windows WIC RAW`。
- 在同一窗口选择内置 LibRaw 后（初次实现，现已移除此选项），新的 full artifact 记录 `LibRaw`，图片正常显示，
  设置文件保存 `builtIn`。设置界面截图已检查，无裁切；资源和图片可见性由真实
  WebView 检查，非后端缓存文件推断。
- 安装按钮、网页入口、返回焦点重新检测、重启提示、错误反馈、仅失效 RAW full
  与重试保留图片有组件回归测试。未实际重装扩展或验证商店下载；受限环境的完整
  桌面进程因既有日志插件目录权限无法启动，缺失场景以原生管线及组件测试覆盖。

以下检查在接入后通过，后续细节修正补跑了受影响的测试和 Clippy：

- release 基准构建通过；真实系统 codec 上的解码和写缓冲重复测量已完成。
- `cargo fmt --all --check` 通过。
- `pnpm check`、`pnpm test`（216 项，随后新增图片保留测试通过）和 `pnpm build` 通过。
- `cargo clippy --workspace --all-targets --features oxy-media/bench-tools -- -D warnings` 通过。
- `cargo test --workspace -- --test-threads=1` 通过；fixture 缺失时跳过的测试不算作
  本次真实 RAW 基准证据，真实数据以以上独立后端运行记录为准。
- 单独对 `oxy-media` 的 Clippy 会触发已有 parser 在部分 feature 关闭时的
  `needless_return` / `ptr_arg` 诊断；使用 workspace feature 合并验证完整工作区，
  没有为此次研究修改 parser 或添加 lint 豁免。

### 固定回退链收敛后的验收

- 删除后端选择、偏好配置及对应命令，设置仅负责状态、安装引导和重新显影。
  RAW planner 固定 WIC → LibRaw；缓存使用通用精确产物身份，前端重试协调只失效当前 full。
- sample-2 冷生产管线实测：WIC 5.45 秒；系统扩展无法实例化的隔离环境自动回退
  LibRaw 9.02 秒。均为 `4688×7028 / native / Satisfied`，两者使用相同的 full
  产物要求，实际后端仅记录在 facts/diagnostics；每次显式重试具有独立生成标识。
- release WebView 的 RAW 设置无后端选择项。sample-1 两次重新显影从入队到完整图
  加载分别约 3.71 秒、3.62 秒，落盘 facts 均为 `Windows WIC RAW`。第二次重试
  期间及前后的 50 ms DOM 采样共 840 次，未发现完整图消失，缩略图保留数始终为 2。
- `pnpm check`、216 项前端测试、`pnpm build`、workspace Rust 测试、格式检查和
  含 bench-tools 的 workspace Clippy 全部通过。原生 release 构建通过。
  未实际重装系统扩展；安装交互由组件测试覆盖，缺失后的回退由上述原生管线验证。

## 重现

```powershell
cargo build --release -p oxy-media --features bench-tools --bin raw_backend_bench

./scripts/perf/raw-backend-bench.ps1 -Sources @('<sample-1.ARW>', '<sample-2.ARW>') `
  -OutputDirectory tests/perf/.reports/raw-decode -Runs 5 -DecodeOnly

./scripts/perf/raw-backend-bench.ps1 -Sources @('<sample-1.ARW>', '<sample-2.ARW>') `
  -OutputDirectory tests/perf/.reports/raw-jpeg -Runs 3

./scripts/perf/raw-backend-bench.ps1 -Sources @('<sample-1.ARW>', '<sample-2.ARW>') `
  -OutputDirectory tests/perf/.reports/raw-buffered -Runs 3 -BufferedJpeg
```

每次使用新的输出目录，防止读取旧结果。`--decode-only` 通常只计时；给单次命令指定
`.png` 输出路径会额外导出像素用于画面检查，PNG 写入不计入 decodeMs。
外部每 25 ms 读取进程的峰值工作集计数，报告为 sampled peak working set；不把它当作
精确瞬时内存、浏览器内存或应用总内存。

接入后，默认 JPEG 基准明确复现旧的无缓冲 baseline，`-BufferedJpeg` 调用正式
缓冲 writer。`wic-qualified` 检查生产 WIC 资格；`pipeline-auto`
使用显式重试标识跑实际 full/cache 管线（输出参数为新的缓存目录），
`pipeline-camera` 保留常规最大相机 JPEG 路径。这些入口均仅在 `bench-tools` 中构建。

严格接口探测：

```powershell
./target/release/raw_backend_bench.exe wic '<sample.ARW>' '<new-output.jpg>' --decode-only
```

官方接口背景：[Implementing IWICDevelopRaw](https://learn.microsoft.com/en-us/windows/win32/wic/-wic-imp-iwicdevelopraw)。
可选系统扩展：[Raw Image Extension](https://apps.microsoft.com/detail/9nctdw2w1bh8)。
