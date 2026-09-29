# E3–E6：原型、邻居与守卫合并的后续验证

2026-09-23。承接 [E0/E1](2026-09-23-anonymous-e0-e1.md) 和
[E2 质量消融](2026-09-23-anonymous-e2-pipeline.md)。只研究**后期完整池匿名聚类**，
继续使用 143 张已确认照片的冻结快照、ArcFace/OSNet 向量、同一 5 秒连拍划分、
人工辅助/自动框两条路由。没有重新训练/下载模型，没有修改标注或主应用。
人工身份只进入 calibration 阈值选择、validation 方法选择和外部评分；
聚合函数不接收身份标签。原 test 已在前两轮被观察，以下仍是探索性结果。

## 受控方法

- **E3**：先用 E0 互近邻图产生核心簇，再把未归组节点用单模态原型吸附。
  比较均值、质量加权人脸均值、medoid、双代表原型；body 没有经校准的质量分，
  因此其加权原型退化为普通均值。候选与次候选相差至少 0.08，不能同图冲突。
  原型只由 stage-1 核心构造，吸附节点不更新原型。
- **E6**：在 E3 后尝试合并核心簇。要求两侧各至少两个不同核心节点支持、
  跨簇最低分数在阈值下 0.12 内、无同图冲突；可靠人脸明显冲突时拒绝。
  吸附节点不能为后续核心合并提供支持。保存每步左右簇、分数、支持边数，
  再由外部评分器计算这一步新增的同人对和错误对。
- **E4**：仅在同一 stage-1 核心簇内，把互为 2/5 近邻的原始 face/body 特征
  做一次强度 0.25 的归一化集中化，保留原向量。它是 NFC-like 消融，
  **不是** Pose2ID 的完整复现。
- **E5**：取原始分数的 5/10 近邻并集作为候选边，在稀疏图上混合原分数与
  Jaccard 邻域重叠（0.25/0.5）。它是图上下文对照，**不是** Cheb-GR 实现。

核心阈值、吸附阈值、合并阈值及重排阈值均在 calibration 选择；
validation 决定原型类型、是否启用 E6 或 E4/E5 变体。E4/E5 的方法选择还要求
validation 匹配覆盖和 B-cubed recall 各不低于原分数 0.05 以上，且误合并对不增加；
否则零误合并的全单例解会错误获胜。test 只评分。
所有方法都保留检测缺失节点和待定节点。背影按原有 3/6/7/9 块分别整块留出。

## E3/E6：核心来源决定风险

先固定 ArcFace/OSNet 70/30 分数构造核心，再做原型/守卫合并：

| 框路由 | validation 选中 | 主 test 匹配覆盖 | 主 test B-cubed F1 | 误合并对 | 漏同人对 |
|---|---|---:|---:|---:|---:|
| 人工辅助 | 原融合图核心，无吸附/合并 | 81.8% | 0.866 | 0 | 74 |
| 自动框 | 原融合图核心，无吸附/合并 | 81.8% | 0.844 | 13 | 74 |

四种 E3 原型在 calibration 不是没有被测：人工辅助路线所有阈值都未吸附节点；
自动框路线低阈值吸附 1 个节点，但未增加已评分人物覆盖。
E6 在 calibration 的人工辅助路由执行 2 次正确合并，匹配覆盖由 79.7% 到 94.6%，
漏同人对由 259 减至 98；validation 和 test 都没有合并，故没有独立收益。
自动框路线在 calibration 执行 1 次正确合并；到了 validation，
同一规则把 7 人簇和 18 人簇合并，尽管有 105 条支持边，仍新增 **39 个错误对**，
总误合并由 151 增至 190。validation 因此拒绝 E6。
这证明“支持边很多”不能替代核心簇纯度检查。

另以 **ArcFace-only 核心**重复相同 E3/E6 流程，避免体貌特征先污染核心：

| 框路由 | calibration 最低阈值 E3 吸附 | validation E3 均值吸附 | validation 选择 | 主 test 匹配覆盖／误合并 |
|---|---:|---:|---|---:|
| 人工辅助 | 9 个，无新增错误对 | 7 个，新增 2 个错误对 | 保留核心；E6 合并曾改善 validation | 81.8%／0 |
| 自动框 | 11 个，无新增错误对 | 16 个，新增 29 个错误对 | 保留核心，不启用 E6 | 78.8%／0 |

人工辅助 face-core E6 在 validation 将匹配覆盖 60.0% 提至 66.7%，零新增错误；
主 test 没有可执行合并，仍为 81.8%。自动框 face-core E3 的双代表原型在
validation 覆盖可达 77.8%，但仍有 13 个错误对，不能用覆盖率单独晋级。
这是**calibration 成功、validation 失败**的直接证据。

### 背影整块留出

四折所选方案中，每个 C 实例仅作为 test 一次：

| 核心来源 | 人工辅助 C 匹配 | 自动框 C 匹配 | 自动框风险 |
|---|---:|---:|---|
| 70/30 融合核心 | 15/15 | 15/15 | 块 6/7 分别 16/39 个全折误合并对 |
| 人脸核心＋选中 E3/E6 | 5/15 | 2/15 | 块 7 仍有 8 个全折误合并对 |

纯人脸核心把大量 C 节点留空；用 body 原型吸附虽能在部分折补回，
但所选规则不稳定。融合核心的 C 覆盖高，却保留了 E2 发现的自动框污染。
这两列只是不同留出折的唯一 C 实例计数；不是跨场次性能估计。

## E4/E5：邻居修正与候选图

仍以 70/30 融合图生成 stage-1 核心。下面列主 test 的固定变体结果；
`候选同人对覆盖`表示跨照片、已知同人 pair 有多少仍存在于图中。
raw/NFC 使用全可用配对，自动框的 92.8% 包含缺检测造成的不可用 pair。

| 框路由／方法 | 匹配覆盖 | 误合并对 | 漏同人对 | 候选同人对覆盖 |
|---|---:|---:|---:|---:|
| 人工辅助 raw | 81.8% | 0 | 74 | 100% |
| 人工辅助 NFC-like k=5 | 81.8% | 0 | 74 | 100% |
| 人工辅助 Jaccard k=10 | 57.6% | 0 | 149 | 76.7% |
| 自动框 raw | 81.8% | 13 | 74 | 92.8% |
| 自动框 NFC-like k=2 | 81.8% | 13 | 74 | 92.8% |
| 自动框 Jaccard k=10 | 60.6% | 0 | 149 | 66.3% |

validation 在人工辅助路由只允许 raw 晋级；NFC-like 两种配置均产生 54 个
误合并对，Jaccard 则严重过度拆分。自动框路由的 NFC-like 在 validation
把误合并从 151 减至 97，但主 test 与 raw 相同；因召回门槛，
validation 最终选 NFC-like k=2，**不能据此声称 test 获得增益**。
Jaccard k=10 在自动框 test 消除 13 个误合并对，同时丢失约三分之一同人候选对；
这是候选生成阶段的召回失败，不只是重排权重问题。
四个背影留出折的 NFC-like 所选方案仍保持 C 15/15，
自动框块 6/7 的 16/39 个全折误合并对没有改善。

## 阶段性决策

1. E3/E4/E5/E6 **均未通过本轮晋级门槛**。不要把本轮后处理设为默认。
2. 下一步先研究候选生成与核心纯度：分别量 face/body top-k 的 recall 和
   clothing-hard-negative 精度，增加跨场次、换装、少数身份和更多背影真值。
   E6 应在合并前检查每个核心的模态一致性与内部半径；单看跨簇支持边不够。
3. 只有候选召回得到保障后，才继续测试稀疏图重排和 2k/5k/10k 的真实规模曲线。
   当前 166 个标注实例无法验证大图的运行时间、峰值内存或产品 UI。
4. 对“识别身份”产品目标，匿名 session 簇仍需与历史具名图库、人工确认和拒识
   另行验证。本轮没有跨场次身份识别结论。

## 复现与检查

- [E3/E6 脚本](../../model-research/scripts/anonymous_e3_e6.py)、
  [E4/E5 脚本](../../model-research/scripts/anonymous_e4_e5.py)
- [融合核心主结果](../../model-research/outputs/anonymous-e3-e6-20260923/results.json)、
  [人脸核心主结果](../../model-research/outputs/anonymous-e3-e6-facecore-20260923/results.json)、
  [E4/E5 主结果](../../model-research/outputs/anonymous-e4-e5-20260923/results.json)
- 每个输出目录另有 `back-holdout.json` 与 `provenance.json`，记录折矩阵、
  脚本/冻结输入哈希、选择纪律；选中方案保存匿名分组和合并审计。

在 `model-research` 中执行：

```powershell
.venv/Scripts/python.exe scripts/anonymous_e3_e6.py --root outputs/anonymous-e0-e1-20260923 --output outputs/anonymous-e3-e6-20260923 --core-source fused --back-holdout
.venv/Scripts/python.exe scripts/anonymous_e3_e6.py --root outputs/anonymous-e0-e1-20260923 --output outputs/anonymous-e3-e6-facecore-20260923 --core-source face --back-holdout
.venv/Scripts/python.exe scripts/anonymous_e4_e5.py --root outputs/anonymous-e0-e1-20260923 --output outputs/anonymous-e4-e5-20260923 --back-holdout
```

研究环境全套 Python 测试 **69 passed**；新增脚本和测试 Ruff 检查通过。
所有代码只读冻结照片/特征，写入忽略的 `outputs/`；没有桌面应用验证。
