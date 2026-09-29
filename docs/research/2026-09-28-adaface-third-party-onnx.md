# AdaFace 第三方 ONNX 与官方权重核查（2026-09-28）

## 结论

存在可直接下载的第三方 ONNX：`yakhyo/adaface-onnx` 发布 IR18、IR50、IR101。
其中 IR101 是 **WebFace12M**；先前项目自行导出的官方 IR101 是 **WebFace4M**。
此前两者向量余弦很低，不能归因为第三方导出失真；比较对象的训练权重不同。

本次第三方 WebFace12M ONNX 与作者本人发布的 WebFace12M safetensors 对照：
204 个可直接按名称对应的 ONNX initializer 全部逐元素相等；32 张冻结真实人脸裁片，
正确处理输入色序后，单位向量最大分量误差 `7.264316e-7`，平均余弦 `1.0`，
两两相似度最大差 `1.132488e-6`。支持将这份固定摘要的第三方 ONNX 用于下载与推理。
这是一致性验证，不是新场次识别质量验收，也没有证明所有输入或所有硬件均等价。

## 来源与产物

- [第三方仓库](https://github.com/yakhyo/adaface-onnx)，核对 commit
  `9310db97e0610ec3858596d599eb8dfa9240e8c7`。README 明确注明 IR101/WebFace12M；
  本次目录树未提供导出脚本，因此不能只凭作者声明确认导出过程。
- [第三方 Release](https://github.com/yakhyo/adaface-onnx/releases/tag/weights) 的 IR101
  文件长度 `260704652`，SHA-256
  `f2eb07d03de0af560a82e1214df799fec5e09375d43521e2868f9dc387e5a43e`。
  GitHub Release API 公布的摘要与本地文件一致。
- [原始官方模型表](https://github.com/mk-minchul/AdaFace#pretrained-models) 分别列出
  WebFace4M 和 WebFace12M。原始 WebFace12M Google Drive ID
  `1dswnavflETcnAuplZj1IOKKP0eM8ITgT` 本次返回下载配额错误，未取得该原始 checkpoint。
- 改用[作者发布的 CVLFace WebFace12M](https://huggingface.co/minchul/cvlface_adaface_ir101_webface12m)，
  固定 revision `54f602a0737bd1ee4a4e7e9fd089a485f397fefd`。
  `model.safetensors` 长度 `260980552`，SHA-256
  `2ea535a43877bd3de8091903935c783ce335be66a9f8917fae9a7a18ae4bbf56`，
  下载后与发布仓库 LFS 元数据核对。
- 另查到同一维护者的 [Hugging Face 镜像](https://huggingface.co/yakhyo/uniface-weights)；
  镜像声明文件与 Releases 相同，但本次主下载路径仍使用 GitHub，并强制校验摘要。
- 还有 [datnt114/adaface-onnx](https://huggingface.co/datnt114/adaface-onnx) 等第三方发布。
  没有对这些额外产物做相同的参数与数值验证，不将它们混作已验证替代文件。

## 实际差异

| 项目 | 第三方 IR101 ONNX | 本地官方 WebFace4M ONNX | 官方 CVLFace WebFace12M |
| --- | --- | --- | --- |
| 训练数据 | WebFace12M | WebFace4M | WebFace12M |
| 发布格式 | ONNX，opset 16 | ONNX，opset 17 | safetensors |
| 输入 | BGR，NCHW，112×112 | BGR，NCHW，112×112 | RGB，NCHW，112×112 |
| 像素归一化 | `(pixel-127.5)/127.5` | 相同 | 相同 |
| 输出 | 512 维，图内 L2 归一化 | 相同 | 网络原始特征；比较时归一化 |
| 动态 batch | 有 | 有 | PyTorch 支持 |

两个 ONNX 的算子类型及数量相同：102 Conv、50 PRelu、49 Add、51 BatchNormalization 等，
均有 458 个 initializer。文件大小接近并不代表训练参数相同；opset 16/17 也不是本次巨大
向量差异的证据。

官方 CVLFace 917 个 state 项去掉 `model.net.` 包装前缀后，能严格加载到本地已校验的
官方 `net.py` IR101 架构中。比较过程不执行下载仓库的 `trust_remote_code`。
204 个名称仍保留的 ONNX 参数与官方 12M 完全一致；其余受常量折叠、Conv/BN 融合和
initializer 重命名影响，未做逐项反融合证明，所以不能写成“全部参数逐位相同”。

## 数值实验

复用 E0/E1 冻结的 112×112 BGR uint8 人脸裁片，按排序均匀取 32 张；在同一 CPU 环境运行，
每个引擎 4 线程、batch 4。各输入文件摘要、模型摘要、脚本摘要保存在 JSON 报告。

| 相对于第三方 ONNX | 平均余弦 | 最低余弦 | 单位向量最大分量误差 | 两两相似度最大差 |
| --- | ---: | ---: | ---: | ---: |
| 作者 12M，按 RGB 契约输入 | 1.0 | 1.0 | 7.264316e-7 | 1.132488e-6 |
| 作者 12M，故意错误输入 BGR | 0.929938 | 0.654436 | 0.120043 | 0.204632 |
| 官方 4M ONNX，正确输入 BGR | -0.000345 | -0.086472 | 0.258840 | 0.171822 |

浮点计算偶尔得到略大于 1 的余弦（最大 `1.000000119`），属于数值舍入。
错误色序对部分脸影响很大；适配器必须遵循实际文件的输入契约，而不是根据模型家族名称猜测。

先前同场次探索性报告在阈值 0.400 时：第三方 12M 为 360/396 同人有向配对、18 个异人返回；
官方 4M 为 352/396、18 个异人返回。它们来自已经多轮观察的 test，不能作为独立生产验收，
也不能推出 12M 在所有场景都更好。配对数不是不同照片数。

## 对桌面下载的影响

选用固定摘要的第三方 WebFace12M ONNX，界面注明“官方训练权重的第三方 ONNX 导出”。
用户无需安装 PyTorch 或执行转换脚本。保留本地导入，但只接受相同固定摘要。
给它独立的 feature-space ID，绝不混用 WebFace4M 向量；不自动确认身份。
下载后仍检查大小与 SHA-256；用户当前非商业研究范围不变。

代码仓库的 MIT 标记不自动解决所有训练数据和检测模型的使用条件；SCRFD 仍按
InsightFace 的非商业研究范围处理。本次没有做新的商业授权判断。

复现（在 `model-research` 环境）：

```powershell
.\.venv\Scripts\python.exe scripts/compare_adaface_exports.py --output outputs/adaface-source-comparison-20260928 --count 32
```

脚本：`model-research/scripts/compare_adaface_exports.py`。
原始结果：`model-research/outputs/adaface-source-comparison-20260928/report.json`。
模型、真实裁片及原始运行结果保留在忽略目录，不提交到 Git。
