# Rust、Windows ML 与 tract 的 ONNX 后端调查（2026-09-24）

## 结论

`tract-onnx 0.23.7` 能在本机 Windows x64 上加载并运行当前 SCRFD-10G 与 AdaFace IR-101 ONNX 文件，输出形状与 ONNX Runtime 一致，零输入数值差很小。因此它可作 **纯 Rust、CPU 优先的实验后端**。但 `tract` 是独立推理引擎，不能把 `tract-onnx` 当成 Windows ML 的 Rust 包装器；当前上游列出的 GPU 后端是 Metal 和 CUDA，没有 Windows ML、DirectML 或 Windows NPU EP 接口。当前短测中 SCRFD 的 tract CPU 耗时远高于 ONNX Runtime CPU，故**不把 tract 定为人物流水线唯一或默认的正式后端**。这项判断在真实照片、线程配置和完整检测后处理基准完成后可重新评估。

若要求 OxyViewer 在 Windows 上用 Windows ML 的 CPU／DirectML，已验证的 Rust 路径是固定版本的 Windows ML 自包含运行时 + `pykeio/ort 2.0.0-rc.13` 的动态加载（见下文）。`ort` 包装 ONNX Runtime C API；Windows ML 厂商 EP、正式安装包与完整人物流水线仍需验证。不要将研究环境的 Python `onnxruntime.dll` 当作产品运行时。

## 官方接口与部署事实

- [Windows ML ONNX API](https://learn.microsoft.com/en-us/windows/ai/new-windows-ml/use-onnx-apis) 提供随包的 ONNX Runtime，列出 C#、C++/WinRT、C/C++ 和 Python 用法；C 头文件是 `onnxruntime_c_api.h`。微软 [CMake 自包含示例](https://learn.microsoft.com/en-us/windows/ai/new-windows-ml/distributing-your-app) 使用 `Microsoft.Windows.AI.MachineLearning` NuGet 包的 `build/cmake`、`WindowsML::Api` 和 `WindowsML::OnnxRuntime` 目标。对 Rust 需要自行验证头文件版本、ABI、符号加载与 DLL 随包部署。
- 截至本次查询，[当前正式版](https://learn.microsoft.com/en-us/windows/ai/new-windows-ml/onnx-versions) 为 `Microsoft.Windows.AI.MachineLearning 2.4.89`（2026-09-22），包含 ONNX Runtime `1.27.1`；`2.6.74-rc` 是候选版。版本会变化，实施时重新核对并固定包版本及摘要。
- [自包含部署](https://learn.microsoft.com/en-us/windows/ai/new-windows-ml/distributing-your-app) 可供未打包应用与 MSIX 使用，不要求目标机器预装共享运行时；CPU、DirectML 路径的相关 DLL 是 `Microsoft.Windows.AI.MachineLearning.dll`、`onnxruntime.dll`、`DirectML.dll`，合计约 41 MB。该文档说明 C/C++ 的框架依赖部署当前不受支持。OxyViewer 的 Tauri／NSIS 包装属于未打包应用场景，但实际 DLL 搜索路径、架构对应、签名与发行包完整性仍待样机验证。
- [Windows ML 入门](https://learn.microsoft.com/en-us/windows/ai/new-windows-ml/get-started) 列出 x64／ARM64，CPU 不需要额外 EP 配置。[内置 EP](https://learn.microsoft.com/en-us/windows/ai/new-windows-ml/supported-execution-providers) 是 CPU 和 DirectML（legacy）；厂商 EP 按需取得，要求 Windows 11 24H2 build 26100 及相应设备／驱动，并需核对各自许可。本机由 `Environment.OSVersion` 报告 `10.0.26200.0`、x64；这仅满足版本方面的候选条件，未验证 Windows ML 包、GPU／NPU 驱动或任何 EP 可用性。
- [EP 选择](https://learn.microsoft.com/en-us/windows/ai/new-windows-ml/select-execution-providers) 建议先显式枚举 `GetEpDevices` 并选择 EP；设备集合可能因驱动和 EP 更新变化。自动设备策略是下一阶段实验，而非先假定最优。[DirectML 约束](https://onnxruntime.ai/docs/execution-providers/DirectML-ExecutionProvider.html) 包括关闭 memory pattern、使用顺序执行，同一 session 不并发 `Run`；动态 batch 形状固定化可改善其优化机会。
- [微软样例索引](https://learn.microsoft.com/en-us/windows/ai/new-windows-ml/samples) 有 C++ CMake、未打包、自包含应用实例，没有 Rust 实例。上游 [ONNX Runtime Rust 目录](https://github.com/microsoft/onnxruntime/blob/main/rust/README.md) 自称实验性且不完整。第三方 [`ort` Rust 项目](https://github.com/pykeio/ort) 包装 ONNX Runtime；本次已验证其在独立样机中能加载 Windows ML `2.4.89` 包的 DLL 并使用 CPU／DirectML。

## Windows ML 官方包与 `pykeio/ort` 实测

从官方 NuGet flat container 下载 `Microsoft.Windows.AI.MachineLearning 2.4.89` 到临时目录；本地 `.nupkg` SHA-256 为 `5c68ecfb947223267abf159a023f5192ad42725e4e9cc995e7c1470dd54dff63`，包内 x64 `onnxruntime.dll` 为 `1.27.1`，本地 DLL SHA-256 为 `6c916621c1807eba630388868bff8eb14ad0f33e1bcc9718b9fa17464fc9368eb14ad0f33e`。该包的 `onnxruntime_c_api.h` 声明 C API 27；[`ort 2.0.0-rc.13` 默认 API 27](https://docs.rs/crate/ort/2.0.0-rc.13/features)。同包 `DirectML.dll` 与 `onnxruntime.dll` 放在临时目录。没有修改仓库依赖、应用 DLL、系统安装或用户模型。

临时 Rust Release 样机使用固定版 `ort = "=2.0.0-rc.13"`，关闭默认功能，只开 `std`、`load-dynamic`、`directml`、`api-27`、`ndarray`；先调用 [`ort::init_from(绝对 DLL 路径)`](https://docs.rs/ort/latest/ort/environment/fn.init_from.html)。DirectML 明确选择设备 ID 0、注册失败即报错、关闭 memory pattern 与并行执行；CPU 和 DirectML 都设 intra-op 线程数 4。C++ 原生对照直接链接同一包的 `WindowsML::OnnxRuntime`，同样设备 ID 和线程配置。模型输入为确定性的 `sin(i × 0.001)` f32 张量；每个独立进程创建 session，记录第一次 `Run`、其后 12 次中的最后 10 次热推理。Rust 与 C++ 的 SCRFD 第一输出元素求和分别为 `43.512720` 与 `43.5127`，AdaFace 都约 `0.663316`；本次没有计算全部输出的误差。

| 模型／配置 | 原生 C++ 热推理中位数 | `ort` Rust 热推理中位数 | `ort` Rust session 创建 | `ort` Rust 首次 Run |
| --- | ---: | ---: | ---: | ---: |
| SCRFD-10G，CPU，4 线程 | `100.70 ms` | `100.99 ms` | `184 ms` | `109 ms` |
| SCRFD-10G，DirectML 设备 0 | `4.20 ms` | `3.85 ms` | `874 ms` | `1043 ms` |
| AdaFace IR-101，CPU，4 线程 | `839.00 ms` | `822.46 ms` | `934 ms` | `916 ms` |
| AdaFace IR-101，DirectML 设备 0 | `6.84 ms` | `7.21 ms` | `1276 ms` | `1149 ms` |

DXGI 枚举的设备 0 是 NVIDIA GeForce RTX 4090（设备 1 为 Intel UHD Graphics 770）。SCRFD 的 DirectML session 发出少数节点留在 CPU 的告警；其算子实际分配仍需 profiling 确认。`ort` 和原生结果接近，说明 Rust 包装层在此小样本中没有明显增加 `Run` 耗时。此测量不含 JPEG／RAW／HEIF 解码、预处理、检测后处理、向量写库、并发作业和前台 UI 争用，也未在干净机器上测试 NSIS 包；首图约 1–1.3 秒的 session 创建加 1–1.2 秒首次 DirectML `Run` 必须纳入产品体验。

### 真实 JPEG 的 Release 阶段短测

2026-09-24 用 `model-research/datasets/dataset01/images/DSC00835.jpg`（6336×9504）在 Release 单进程测试中加载同一组本地研究模型及 Windows ML 2.4.89。测试先以 `image::open` 完整解码，再缩至 1200×1800，SCRFD 使用 640 画布、分数阈值 0.5、NMS 阈值 0.4；此图检测到 1 张脸。每个 provider 丢弃首轮后重复 10 次并取中位数；检测计入 SCRFD 预处理、`Run` 和框／五点后处理，编码计入五点对齐、`Run` 和 L2 归一化。

| Provider | 完整 JPEG 解码 | 缩至分析图 | 检测中位数 | 单独五点对齐中位数 | 单脸编码中位数 |
| --- | ---: | ---: | ---: | ---: | ---: |
| DirectML 设备 0，4 线程 | 295.38 ms | 500.81 ms | 26.71 ms | 0.31 ms | 2.90 ms |
| CPU，4 线程 | 299.73 ms | 476.10 ms | 148.58 ms | 0.36 ms | 2235.84 ms |

这是单张真实照片的阶段测量，不是分布统计或交互延迟。该测试使用通用 `image::open` 全尺寸解码，未使用产品的有界 native JPEG 输入路径，也没有在此测试应用缩放方向、ICC、RAW／HEIF、SQLite 写入、后台争用或前台可见行为。CPU AdaFace 在此路径明显慢于前述确定性张量 `Run` 短测；单独五点对齐只占约 0.36 ms，差异主要留在 ORT 推理路径，仍需 profiling 核对模型算子、输入及线程行为。

另外用 `ort::ep::DirectML::default()` 验证了新版 DML2 注册路径：SCRFD 成功运行，热推理中位数 `4.19 ms`；该路径未显式指定设备，不能把它的时间直接归因于 DXGI 设备 0。上表的设备 0 路径调用旧的 `SessionOptionsAppendExecutionProvider_DML`；正式接入应优先选择有明确设备过滤／性能偏好的 DML2 路径，并记录实际设备。

`ort` 与 `ort-sys 2.0.0-rc.13` 都声明 Rust 最低版本 `1.88`，低于当前本机 `rustc 1.96.0`；仓库声明的最低版本已由 `1.85` 提升到 `1.88`。`ort` 的默认功能会自动下载二进制，产品依赖已关闭默认功能，以便只使用受控的 Windows ML 包。`2.0.0-rc.13` 仍是预发布版本；需固定精确版本、审查依赖／许可、把 DLL 与应用一起验证，再纳入主应用。

### 接入进展

`oxy-people/onnx_face.rs` 现已包含共用的 `ort` 人脸适配器，Windows 分支使用 Windows ML 运行时。它要求完整可安装的产品清单和已校验的模型文件；对应用提供的 `onnxruntime.dll` 以及 DirectML companion DLL 校验 SHA-256，再显式加载。CPU／DirectML session 均开启图优化，DirectML 关闭 memory pattern 和并行执行且要求 EP 注册失败报错；检测输入借用现有张量缓冲区，核验 SCRFD 九组输出形状与有限值。九组输出按 8／16／32 步长、每格两个锚点解出框与五点，映射回源图坐标，按 InsightFace 的含端点 IoU 计算执行稳定分数排序和 NMS；不截断多人检测数量。编码器只接受 512 维有限输出并做 L2 归一化。仓库最低 Rust 版本现已从 `1.85` 提到 `1.88`。

检测适配器现可将框裁至分析图边界，生成与输出顺序无关的缓存 ID，并将五点按分析图宽高归一化后交给检测缓存版本 2；编码时再按同一朝向的分析图还原像素五点。缓存版本变化只重建派生检测表，人工框与审阅事实仍独立保留。此转换仍未接入正式后台 worker，也未用真实照片对照 Python 后处理的完整输出。

验证：`cargo check -p oxy-people` 与 `cargo check -p oxyviewer` 通过；`cargo test -p oxy-people -p oxy-library` 为 18＋82 通过，分别忽略 1＋2 个环境依赖测试。合成双锚点／多脸用例覆盖坐标解码和 NMS，检测缓存测试覆盖归一化五点读写与旧派生缓存重建；研究模型测试分别在 CPU 和 DirectML 上以本机官方 Windows ML 2.4.89 DLL、SCRFD／AdaFace 权重手动执行并通过；`cargo clippy -p oxy-domain -p oxy-library -p oxy-people --all-targets -- --no-deps -D warnings` 通过。完整 `cargo clippy` 目前先被已有 `oxy-fs/external_apps.rs` 的两个 `collapsible_if` lint 阻断。此验证覆盖适配器张量路径和基本后处理，不证明正式安装包、真实照片识别质量或完整人物分析作业。

## tract 实机验证

独立临时 Cargo 项目（未修改主应用依赖）使用 `tract-onnx = "=0.23.7"`、Rust `1.96.0`、`cargo run --offline --release`。输入全零 `f32`，AdaFace `[1,3,112,112]`，SCRFD `[1,3,640,640]`；显式固定输入形状后 `into_optimized()`、`into_runnable()`、`run()`。对照由研究环境 Python `onnxruntime 1.30.0` 的 CPU EP 在同机同输入上运行。模型仅是本地研究样本，输出对照不授权产品分发权重。

| 模型 | tract 输出 | 与 ORT CPU 的全部元素最大绝对差 | tract CPU 单次推理，4 次 | ORT CPU 单次推理，4 次 |
| --- | --- | ---: | ---: | ---: |
| AdaFace IR-101 | `[1,512]` | `2.43e-6` | 约 `459–578 ms` | 约 `322–346 ms` |
| SCRFD-10G | 9 个输出：3 组置信度、框、关键点 | 最大 `5.49e-6` | 约 `698–830 ms` | 约 `42–47 ms` |

以上是单机、单次零输入、极少重复的方向性观察。两个引擎的默认线程数、内存分配和图优化选项未对齐，进程与模型加载也不同；不应将表中比值称为正式吞吐结论。完整照片检测召回、后处理、批处理、冷／热延迟、后台任务与前台预览争用尚未验证。不过 SCRFD 差距足以要求在选择 tract 前做配置和真实性能调查。

`tract-onnx 0.23.7` 声明 `rust-version = "1.91"`，本仓库当前声明 `1.85`。用户已允许升级 Rust 版本，因此 MSRV 不再是不可逾越的限制，但引入前须同步更新 `Cargo.toml`、`CONTRIBUTING.md`、CI 和各平台构建环境，并检查所有目标平台。[tract 当前 README](https://github.com/sonos/tract/blob/main/README.md) 建议新客户端使用 `tract` 公共 facade，而不是直接依赖不稳定的 `tract-onnx` 内部 API；实际选型时也需重审版本与公共接口。

## 对 OxyViewer 的接入建议与验收

1. 在 `oxy-people` 保持模型推理适配器边界，沿用现有 `face_input.rs` 与 `oxy-media` 的有界输入；不要让 Windows ML／tract 类型进入 `oxy-domain` 或 Tauri IPC。Windows 推理在后台专用 worker 中进行，仍受取消、源版本、内存预算、stage 指纹与作业 generation 约束。
2. Windows 正式候选采用已测通的 `ort` 动态加载 + Windows ML `2.4.89` 自包含 CPU／DirectML。先把临时样机转为 `oxy-people` 的受控适配器，固定合法模型文件的哈希与输入输出契约，并核对 SCRFD／AdaFace 全部输出和 CPU 回退。Windows 11 厂商 EP 后续单独评估，不能根据设备名推断算子实际落在 GPU／NPU。
3. 若保留 tract，限定为独立 CPU 候选，并先按上游公共 API、单 session 并发限制和线程配置重测。`tract` 没有给 Windows ML 增加硬件 EP；双运行时会增加包大小、测试矩阵和结果指纹复杂度，必须有可测的回退价值才引入。
4. 用真实 JPEG／RAW／HEIF/HIF 混合文件夹，测冷启动、每图各阶段、整夹吞吐、CPU／GPU／内存峰值、前台 Grid／Loupe 响应和正式 NSIS 包在干净机器上的 DLL 加载。只有实际完成这些验证，才可更新 [人物流程](../PERSON_WORKFLOW.md) 中的运行时状态。模型下载许可与运行时许可分别审查；AdaFace 权重目前仍不在产品可信安装清单。
