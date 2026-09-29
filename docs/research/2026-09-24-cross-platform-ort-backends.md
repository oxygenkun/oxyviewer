# Windows 与 macOS 的 ORT 人脸推理后端预设计（2026-09-24）

## 共用边界

`oxy-people/onnx_face.rs` 在 Windows/macOS 编译同一套 SCRFD、AdaFace 输入输出约束、多人检测后处理、缓存几何和 L2 特征校验。`PersonModelArtifact`、stage fingerprint 与 feature-space ID 不含 provider 名称；同一模型权重和预处理仍需跨设备数值对照，若差异影响阈值则分开校准。ONNX Runtime 的 CPU 是明确可选择的配置，硬件 EP 不可用或运行失败时由后台作业重建 CPU session；失败不得被解释成“照片无人”。前台打开文件夹和 UI 线程不创建 session 或执行推理。

| 平台 | 运行时来源与验证 | 当前 `OnnxProvider` | 现有证据 |
| --- | --- | --- | --- |
| Windows | 应用提供的 Windows ML `onnxruntime.dll`，按应用固定的 SHA-256 校验；DirectML 另校验相邻 DLL | CPU、指定设备 ID 的 DirectML | 本机 Windows ML 2.4.89 CPU／DirectML 运行过 SCRFD 和官方 AdaFace 导出；尚无正式包 |
| macOS | 应用提供的、C API 与 `ort 2.0.0-rc.13` `api-27` 匹配且编入 Core ML EP 的 `libonnxruntime.dylib`；按应用固定的 SHA-256 校验 | CPU、Core ML `All`／`CPUAndGPU`／`CPUAndNeuralEngine` | 仅代码预接；当前 Windows 主机无 Mac 目标工具链与 Mac 实机结果 |

`ort` 的平台依赖均固定在 `oxy-people/Cargo.toml`：Windows 启用 `directml`，macOS 启用 `coreml`，两者用 `load-dynamic` 与 `api-27`。运行时动态库、模型和 DirectML companion 各自校验，不把研究环境 Python 包的运行时当成发行输入。进程只初始化一个绝对路径对应的 ORT 动态库；检测与编码 session 可分别选 provider，但需由 worker 记录实际选择、失败与回退原因。`ort` session 的可变运行边界使同一 session 的运行保持串行，后台并发用有界 session/任务数控制。

## macOS Core ML 选择

[ORT Core ML 官方说明](https://onnxruntime.ai/docs/execution-providers/CoreML-ExecutionProvider.html)称 Core ML EP 需要 macOS 10.15+，`MLProgram` 需 macOS 12+；预接代码选兼容面更广的 `NeuralNetwork` 格式，并明确设置 `RequireStaticInputShapes=1`。SCRFD 输入画布固定，AdaFace ONNX 的 `batch_size` 在建 session 时覆盖为 1。先允许 `All`、CPU+GPU、CPU+Neural Engine 三种计算单元作实测候选；它们不是 GPU/ANE 实际承载量的证明。Core ML 不支持的节点可能落到 ORT CPU；要在 Mac 上用 profiling／compute-plan 日志检查分配，不能凭 EP 注册成功推断全图加速。

Core ML 编译缓存目录由调用方放在应用可写缓存下，按模型摘要、ORT/Core ML 运行时版本、格式和 provider 配置隔离；模型的安装路径目前以 SHA-256 命名，避免同一路径换权重仍命中旧编译缓存。缓存只存可重建的编译产物，清理遵循容量与版本失效规则。ORT 官方文档说明默认不启用编译缓存，且不会自动追踪同路径模型权重变化；正式接线前须实测冷／热创建成本。Mac 动态库的架构、签名、`@rpath`/加载路径以及 `.app` 打包位置必须在正式构建机与干净机器验证。

## 验证门槛

1. 在 macOS arm64（需要时再加 x64）构建 `oxy-people` 和桌面 Release 包，确认选定 ORT dylib 确实包含 Core ML EP、匹配 C API，并从应用目录加载；故意提供错摘要、错架构和不含 EP 的库应清楚报错。
2. 对同一官方 AdaFace ONNX 与 SCRFD ONNX，分别跑 ORT CPU 和 Core ML，比较九个检测张量、框/五点、512 维特征、余弦排序及未知脸处理；保持特征空间和阈值隔离，直到未见场次验证数值可互用。
3. 用真实 JPEG／RAW／HEIF/HIF 计算冷／热 session 创建、首次编译、整图各阶段、整夹吞吐、峰值内存／能耗、CPU 回退和前台 Grid／Loupe 争用。逐阶段看 Core ML 算子覆盖及 CPU↔设备搬运。若加速路径更慢或资源争用明显，允许该阶段保持 CPU。

当前更改只完成跨平台 Rust 结构和 macOS Core ML session 配置预接。它不表示 Mac 已运行、运行时已打包、模型已可在应用内安装，或完整人物 worker 已启用。
