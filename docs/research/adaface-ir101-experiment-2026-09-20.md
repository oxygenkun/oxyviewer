# AdaFace IR-101 本地实验（2026-09-20）

## 结论

AdaFace IR-101 WebFace12M 的第三方 ONNX 导出可被当前纯 Rust `tract` 运行时加载，能够沿用
YuNet 检测与 5 点 ArcFace 对齐，在真实双人夹具上输出有限且已 L2 归一化的 512 维 embedding。
这证明当前缓存、匹配与聚类边界可以承载第二种 embedding，但**不代表精度已经优于 SFace**。

该模型暂时只作为 debug 本地实验：不复制到 Tauri resources、不进入 release bundle，也不加入
第三方 notices。ONNX 仓库以 MIT 发布转换代码与文件，但 WebFace12M 训练权重的发行和商业使用
边界仍需单独审查；审查完成前不能把本实验描述为可分发模型。

## 固定输入

- 模型：`adaface_ir_101.onnx`，260,704,652 bytes。
- SHA-256：`f2eb07d03de0af560a82e1214df799fec5e09375d43521e2868f9dc387e5a43e`。
- 来源：`yakhyo/adaface-onnx` GitHub release `weights`；该仓库声明模型由官方 PyTorch
  权重导出。
- 输入：112×112、BGR、`(x / 255 - 0.5) / 0.5`。
- 输出：512×`f32`，进入数据库前由共享 analyzer 边界执行 L2 normalization。

`3rdpart/face-models/experiments.json` 固定下载信息。运行：

```bash
pnpm faces:prepare:adaface
OXY_FACE_EMBEDDER=adaface pnpm tauri dev
```

该开关只在 debug 构建生效，并使用进程内 analyzer；默认与 release 继续使用隔离 worker 中的
YuNet + SFace。

## 验证

```bash
cargo test -p oxy-faces \
  adaface::tests::local_ir101_model_loads_and_outputs_512_values \
  --lib -- --ignored --nocapture

cargo test -p oxy-faces \
  adaface_ir101_runs_through_the_full_pipeline \
  --test reference_parity -- --ignored --nocapture
```

两项均通过。第二项覆盖真实夹具的检测、对齐、512 维推理、有限值检查与单位范数检查。

## 尚未证明

- 当前 SFace 阈值不能直接视为 AdaFace 的已校准阈值；应使用真实照片库重新估计 match 与
  cluster threshold。
- 尚未对亚洲女性 hard negatives 计算 wrong-merge、wrong-split、Pairwise F1 或 BCubed F1。
- 尚未测 release 推理吞吐、峰值内存和大图库后台分析耗时。
- 尚未完成权重许可审查，因此没有默认启用或打包。
