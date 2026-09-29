# AdaFace 官方权重来源与 ONNX 导出核对（2026-09-24）

> 2026-09-28 更正：下文第三方 ONNX 与本地官方导出的巨大差异来自 **WebFace12M 与 WebFace4M 不同训练权重**，不能据此判断第三方转换失真。对应官方 WebFace12M 权重的参数与真实裁片输出对照已通过，见 [第三方 ONNX 同源核查](2026-09-28-adaface-third-party-onnx.md)。下文保留当时实验事实，不再代表默认模型选择。

用户明确限定人物模型方案为非商业用途，并要求默认使用 AdaFace 官方来源。此记录把权重**来源**、格式转换和模型特征空间分开核验。当前实现尚未把官方 checkpoint 下载与转换接入桌面安装流程；不得把现有第三方 ONNX 改名冒充官方文件。

## 官方来源

- [AdaFace 官方仓库的预训练模型表](https://github.com/mk-minchul/AdaFace#pretrained-models)列出 R100 WebFace4M checkpoint，Google Drive 文件 ID 为 `18jQkqB0avFqWa0Pas52g54xNshUOQJpQ`。该文件是 PyTorch checkpoint，不是 ONNX。
- 固定官方源码 commit `c60eaa786a42c03444f3df7096dbaf9d57ae010d` 的 `net.py`。源码 SHA-256 为 `b4db4eb0174a385fd29e5f616391b50d443f455990c8b88dcab1f8021af8ba4c`。
- 2026-09-24 下载的 checkpoint 长度 `683066959` 字节，SHA-256 为 `0a379dbaf7d79573d1ff63f317c0296f3adfb637b9c00b6234fdcc53a77a0f61`。以 `torch.load(..., weights_only=True)` 读取后，`state_dict` 有 921 项，其中 917 个 `model.` 参数严格匹配官方 `net.build_model('ir_101')`。
- [CVLFace 官方 IR101 WebFace4M 页面](https://huggingface.co/minchul/cvlface_adaface_ir101_webface4m)还提供 safetensors 路径，但其示例输入为 RGB；本次选用旧 AdaFace 仓库的 BGR checkpoint，不把两种格式、色序或模型向量混用。

本项目当前按用户给定的非商业研究范围使用这些权重。来源和使用限制仍应在模型安装界面清楚展示；未来改变用途时要重新核对适用条件。

`oxy-people` 的产物安装边界已接受受信 manifest 中的 `ResearchOnly` 状态，同时保留摘要、HTTPS、完整 pipeline 校验，并拒绝 `Unresolved`。这只打开非商业研究权重的安装策略；应用还没有可供运行时使用的官方权重 manifest、下载及转换入口。

## 可复现导出

在隔离的 `model-research` uv 环境运行。`oxy-person-download-models` 默认准备 OSNet 和此官方 AdaFace 导出；只需 AdaFace 时运行：

```powershell
cd model-research
uv run python scripts/export_official_adaface.py
```

脚本先验证官方源码与 checkpoint 的固定摘要，然后用 PyTorch 的 legacy ONNX exporter、opset 17、动态 `batch_size` 导出单一 512 维归一化 embedding。对确定性输入比较 PyTorch 与 ONNX Runtime CPU：输出形状 `[1,512]`，最大绝对误差 `5.383e-7`、余弦 `1.0`。本机导出 ONNX 大小 `260708820` 字节，SHA-256 为 `98c1796b98f019b3cfb6c276effbfe5a0c63f209fb5f396e356f6c9335eedf87`。导出产物在 `models/` 下，不提交到主应用或 Git。

同一导出 ONNX 已在 `oxy-people` 的 Windows ML 2.4.89 `ort` 适配器上分别通过 CPU 与 DirectML 张量测试。这验证格式和运行兼容，不是身份识别质量验证。

现有研究 `models/adaface_ir_101.onnx` 下载自第三方 `yakhyo/adaface-onnx`。对同一确定性 BGR 输入，官方导出与该文件的 512 维向量余弦仅 `0.047807`，表明两者不能视为相同特征空间。旧版 one-shot 的 `0.300` 阈值、缓存向量和候选评估不可直接迁移；需用官方导出重新抽取同一照片和参考，并在未见场次重校准。

`scripts/download_benchmark_models.py` 与旧 E0/E1 脚本仍固定第三方文件，供历史实验复现；它们不代表新人物流程的默认权重。

## 冻结 E0/E1 对齐脸的探索性复算

`scripts/evaluate_official_adaface.py` 复用已有 E0/E1 冻结快照的 144 个 SCRFD 五点对齐 BGR 裁片，在 CUDA EP 上批量产生官方权重向量；它在内存中只替换 AdaFace 槽位，保留原始自动／人工辅助实例、资格规则、缺脸分母和标签，结果另存 `outputs/official-adaface-20260924-v2/report.json`，不覆盖旧研究。自动路线 189 行中 134 行有可重算的对齐脸；人工辅助路线 166 行中 141 行有可重算的对齐脸。

| 自动路线阈值 | calibration 同人／异人返回 | validation 同人／异人返回 | 已观察 test 同人／异人返回 |
| ---: | ---: | ---: | ---: |
| 0.300 | 918／68 | 170／44 | 364／180 |
| 0.325 | 918／56 | 168／36 | 364／138 |
| 0.350 | 918／42 | 164／22 | 364／82 |
| 0.400 | 912／8 | 160／4 | 352／18 |
| 0.450 | 890／0 | 148／0 | 342／6 |

自动路线 test 的合格同人有向配对分母仍为 396；其中合格但自动缺脸的实例保留在分母。旧第三方 ONNX 在 0.300 的 test 为 366／144，不能因 0.300 数值相同就认定两套向量或阈值等价。这里的 test 曾在前期研究中反复观察，所有结果均为探索性诊断；正式默认阈值和错候选照片量须在未见场次上重新校准。

## 进入默认安装仍需的工程步骤

1. 桌面受信清单固定官方 checkpoint 的 URL、长度、SHA-256、来源和非商业使用说明；用户触发下载，缓存与失败处理沿用 `oxy-people/artifact_store.rs`。
2. 为桌面提供**可复现、受控**的 checkpoint 到 ONNX 转换或应用发布的同源派生 ONNX 产物，并验证导出摘要与输入输出契约。当前隔离研究脚本不能假装已是桌面运行时依赖。
3. 用官方导出重跑身份检索、现场召回和阈值校准，给它独立的 feature-space ID；再将检测、编码、增量作业、候选和 UI 接线。
