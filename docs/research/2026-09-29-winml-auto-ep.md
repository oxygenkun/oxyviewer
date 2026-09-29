# Windows ML 自动 EP 与真实照片验证（2026-09-29）

## 修改与范围

主应用原先给 SCRFD 和 AdaFace 都传入 `OnnxProvider::Cpu`；下载 Windows ML
运行时本身并不会启用 GPU。现在 `oxy-people/windows_ep.rs` 在后台识别准备阶段
调用固定版本 2.4.89 的 C API，发现认证 EP，异步准备／下载后注册到同一个 ORT
环境，再按实际枚举出的设备建立 session。

设备排序：NVIDIA `NvTensorRTRTXExecutionProvider` GPU → OpenVINO GPU →
其他 GPU → OpenVINO NPU → 其他 NPU → OpenVINO CPU → 内置 DirectML → CPU。
模型建立或两阶段试运行失败都会记录原因并继续尝试；运行中失败仍进入任务失败
记录，不作为空检测成功发布。下载回调提供百分比，取消会调用 WinMLAsyncCancel，
等待原生操作结束后释放异步块，不让旧操作回调引用已经释放的状态。

目录与运行时 DLL 先经过原有固定摘要校验；目录 DLL 保持进程级生命周期。
厂商 EP 包由 Windows 管理、下载及更新，并非应用固定摘要的运行时文件。
SCRFD 的两个 `?` 空间维显式固定到 960；AdaFace 的 `batch_size` 固定为 1。
这修复了 DirectML 动态空间维在 Reshape_223 上的运行时错误。

接口来源：[微软 EP 安装文档](https://learn.microsoft.com/en-us/windows/ai/new-windows-ml/initialize-execution-providers)
以及固定 NuGet 包随附的 `WinMLEpCatalog.h`、`WinMLAsync.h`。Rust 绑定使用
`ort 2.0.0-rc.13` 的 `register_ep_library`、`devices`、`with_devices`。

## 本机 Release 实测

使用已安装、摘要校验通过的应用 SCRFD / AdaFace WebFace12M ONNX，真实照片
`model-research/datasets/dataset01/images/DSC00835.jpg`，检测画布 960。
测试直接解码原图；下表检测时间包含原图预处理，不等于纯设备推理时间，也不等于
主应用经过媒体服务缩放后的端到端吞吐。

| 项目 | 结果 |
| --- | --- |
| 正常用户会话目录准备 | OpenVINO 与 TensorRT RTX 均准备、注册成功 |
| 自动选择 | NvTensorRTRTXExecutionProvider / GPU，device 9860 |
| 首次目录准备及模型加载（未加入试运行的初次探测） | 92.06 s |
| 最终代码再次准备、建模及两阶段试运行 | 11.65 s |
| RTX 真实照片检测及预处理 | 306.88 ms，检测到 1 张人脸 |
| RTX 同照片真实人脸特征提取 | 5.25 ms，512 维，归一化有效 |
| CPU 对照 | 人脸数量一致，同五点输入特征余弦 0.99999917 |
| 受限会话回退 | 厂商目录返回 0x80070520；自动选择 DmlExecutionProvider / GPU |
| DirectML 回退真实照片 | 检测 327.23 ms、1 张人脸、编码 3.04 ms |

以上是单次功能回归，不是统计性能比较或跨场次识别质量证明。未测 Intel GPU/NPU
上的实际 OpenVINO 推理，也未做新的正式安装包／WebView 整夹交互性能验收。

后续同日的 [多后端速度比较](2026-09-29-winml-backend-speed.md) 已覆盖 Intel UHD 770，
发现其 OpenVINO GPU 编码输出与 CPU 不一致；该问题未由本页的成功试运行检查发现。

复现自动准备及 CPU 对照（会通过 Windows 下载缺失的认证 EP）：

```powershell
$env:OXY_TEST_MODEL_ROOT = "$env:APPDATA/app.oxyviewer.desktop/person-models"
$env:OXY_TEST_FACE_IMAGE = "$PWD/model-research/datasets/dataset01/images/DSC00835.jpg"
cargo test -p oxy-people --release installed_models_use_automatic_ep_and_real_image -- --ignored --nocapture
```

## 常规验证

- `cargo test --workspace` 通过；最终人物模块回归 51 passed、3 ignored。
- `cargo fmt --all --check`、人物模块 `clippy --all-targets --no-deps -- -D warnings` 通过。
- 全仓库 Clippy 被既有 oxy-fs / oxy-metadata-parser 等模块的 lint 错误阻止，
  包括 collapsible_if、manual_is_multiple_of；未扩大本次修改范围。
- `pnpm check`、前端 372 项测试、`pnpm build` 通过。测试与构建在受限会话中
  被 esbuild 目录权限阻止，正常权限重跑通过。
