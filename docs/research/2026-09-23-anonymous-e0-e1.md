# E0–E1：现场筛图与后期标注的匿名人物聚类

2026-09-23，在 dataset01 最新整图确认快照上完成本轮 E0/E1 minidataset 实验。
模型只输出人物簇编号，原人工身份只用于外部评分；预测未写回数据集或主应用。

## 范围与结果

143 张照片、166 个有效人物实例，165 个具有已确认人工分组。
用途视觉初审：单图正/侧脸证据 A114、遮挡 B24、背影/转身/服装辅助 C15、待定13；
A 中含1个 unknown。用途审核尚不是新的用户确认，不能只从 quality 标签自动推导。

以下是 **validation 选择的方法** 在固定 test 上的匿名簇一对一匹配覆盖率。
未分组与缺特征均计入分母；不能把它解释为具名身份识别准确率。

| 自动框评估 | 样本 | E0 | E1 | 说明 |
|---|---:|---:|---:|---|
| 现场筛图 | 30 | 80.0% | 80.0% | ArcFace / LVFace-S，按时间增量聚类 |
| 后期主测试 | 33 | 78.8% | 90.9% | ArcFace 图法 / LVFace-S DBSCAN；A30/B3/C0 |
| 后期背影补充留出 | 15 | 66.7% | 40.0% | 四个完整连拍块逐次留出，不能与主 test 混为总分 |

主测试选定方案均无已知人物误合并对，但背影补充实验存在误合并、漏分组和方法选择不稳定。
人工框辅助的后期结果也没有一致改善。因此本轮**不支持整体替换基线**。
同场次、小样本与用途初审的限制均在完整报告披露。

## 完成的实验

- E0：ArcFace、AdaFace、OSNet、固定0.7/0.3融合，人工框与自动框分开；
  DBSCAN eps/min_samples、平均链接、互近邻图与原有质量门控图对照。
- 现场按时间增量聚类；后期完整池聚类。calibration选阈值、validation选方法，test仅评分。
- 匹配/纯簇/已分组覆盖率、B-cubed、ARI/NMI、pair F1/ROC、误合并/拆分、质量桶、耗时与RSS。
- E1：官方固定版本LVFace-T/S，同样检测、对齐、输入、划分；四个人脸编码器CPU/CUDA实测。
  RTX4090实际执行CUDA算子；80份主测试分组文件与CPU逐字节一致。
- 冷提取核心流程406.07秒、峰值RSS约2392MiB；热缓存读取0.190秒，含导入/哈希的热进程9.90秒。
  热缓存不等于推理速度。模型单独进程性能、批量吞吐及数值误差均另存。
- 1/5/10/30秒连拍分组敏感性；10/30秒划分样本不足明确标为未测。
- 背影四折整块留出、全部已标注分母敏感性、匿名审核页及错误对视觉抽查。

CPU单脸中位耗时：ArcFace99.67ms、AdaFace812.56ms、LVFace-T15.58ms、LVFace-S47.95ms。
CUDA对应3.98/7.56/4.82/4.69ms。仅为编码器，不含解码、检测、界面或网络。
检测、对齐和OSNet仍用CPU；本轮没有测量产品UI、人工修复操作数或进程峰值显存。

## 产物与复现

- [完整结果、性能及局限](../../model-research/outputs/anonymous-e0-e1-20260923/report.md)
- [匿名聚类审核页](../../model-research/outputs/anonymous-e0-e1-20260923/review-results/index.html)
- [主评估矩阵](../../model-research/outputs/anonymous-e0-e1-20260923/evaluation/results.json)
- [用途初审](../../model-research/outputs/anonymous-e0-e1-20260923/use-case-audit.json)
- [背影补充实验](../../model-research/outputs/anonymous-e0-e1-20260923/back-holdout/results.json)
- [脚本与输出哈希](../../model-research/outputs/anonymous-e0-e1-20260923/completion-manifest.json)

从 `model-research` 执行，提取、评估、原图法、补充留出分别可复跑：

```powershell
.venv/Scripts/python.exe scripts/anonymous_extract.py --root outputs/anonymous-e0-e1-20260923
.venv/Scripts/python.exe scripts/anonymous_evaluate.py --root outputs/anonymous-e0-e1-20260923
.venv/Scripts/python.exe scripts/anonymous_evaluate.py --root outputs/anonymous-e0-e1-20260923 --provider cuda
.venv/Scripts/python.exe scripts/anonymous_legacy_graph.py --root outputs/anonymous-e0-e1-20260923
.venv/Scripts/python.exe scripts/anonymous_back_holdout.py --root outputs/anonymous-e0-e1-20260923
.venv/Scripts/python.exe scripts/anonymous_report.py --root outputs/anonymous-e0-e1-20260923
```

CUDA在独立 `.venv-e1-cuda` 中运行，依赖锁定清单位于输出目录 `cuda-requirements.txt`。
GPU编码器向量由 `anonymous_model_profile.py --provider cuda --export-all` 生成，再由
`anonymous_cuda_verify.py` 对齐到相同人物实例。既有CPU环境、主应用依赖未修改。

权重来自[LVFace官方固定版本](https://huggingface.co/bytedance-research/LVFace/tree/b12702ab1f5c721748e054a66dc90e1edd1f0724)，
SHA256已与官方LFS记录核对；按[官方推理示例](https://github.com/bytedance/LVFace/blob/main/inference_onnx.py)
使用RGB预处理。许可记录保留于研究目录，本轮没有改变产品模型或部署范围。
