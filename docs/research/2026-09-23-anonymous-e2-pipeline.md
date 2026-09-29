# E2 质量消融与后续人物聚合 pipeline 研究

2026-09-23。承接 [E0–E1 匿名聚类实验](2026-09-23-anonymous-e0-e1.md)。
本轮使用同一冻结快照、ArcFace/OSNet 向量、用途初审、5 秒连拍块和人工真值计分口径。
只改配对分数与聚类器选择；未重新检测、训练新模型或改写标注。原 E0/E1 的 test
已经被观察，本轮 **所有 test 和背影留出结果均为探索性诊断**，不能视作新的独立验收。

## E2 消融到底测了什么

缓存中的 `quality` 是检测置信度 × 脸短边尺寸因子 × 对齐脸 Laplacian 模糊度因子，
取值约 0–1。它不是身份可识别概率，也不是 QCFace 输出。
本轮称为 **E2a 复合质量代理实验**；因缓存没有独立保存检测分、尺寸、模糊度、
关键点残差和遮挡置信度，无法把各因子的效应拆开，更没有完成 E2b/E2c/E2d。
人工 `quality`/`pose` 标签只用于错误解释，未作为 scorer 输入。

固定每路原始向量，用以下分数消融：

| 名称 | 变化 | 缺模态处理 |
|---|---|---|
| `face` | ArcFace cosine | 缺脸为不可用 |
| `face_margin_0.1/0.2` | 两脸最小质量越低，cosine 减去越大边距 | 缺脸为不可用 |
| `fixed_70_30` | E0 的 70% face + 30% body | 单路存在时使用该路 |
| `adaptive_0.3/0.5` | 质量越高，人脸权重越高；权重限于 0.5–0.9 | 单路存在时使用该路 |

现场仅比较人脸三种分数及原 online 聚类，保持时间顺序且不读取未来节点。
后期比较图法、平均链接、DBSCAN min_samples=2；每个组合在 calibration 扫
0.20–0.90 阈值（步长 0.05），validation 按零/少误合并、B-cubed F1、匹配覆盖
依次选算法及分数；test 只报告。所有缺检测的人物仍保留在分母和后期推理池。
`score_variants()` 无身份标签输入。此对照没有重跑 E0 的 DBSCAN min_samples=3，
所以只与其中相同算法配置及本轮 `face`/`fixed_70_30` 控制组比较。

## 主测试结果

| 框/用途 | validation 选出的 E2 分数/算法 | test 匹配覆盖 | B-cubed F1 | 误合并对 | 漏掉同人对 |
|---|---|---:|---:|---:|---:|
| 人工辅助/现场 | `face / online` | 83.3% | 0.889 | 0 | 48 |
| 自动/现场 | `face / online` | 80.0% | 0.860 | 0 | 59 |
| 人工辅助/后期 | `adaptive_0.3 / graph` | 81.8% | 0.866 | 0 | 74 |
| 自动/后期 | `face / graph` | 78.8% | 0.839 | 0 | 87 |

现场两路均回到 E0 ArcFace 原始分数。人工辅助的后期 `adaptive_0.3` 在 validation
选中，但 test 分组和 `fixed_70_30 / graph` 一致；自动框后期直接选了 face 原始分数。
因此没有证据说明当前质量代理改善了主测试的匿名分组。对自动框后期，
`adaptive_0.3 / graph` 在 validation 出现 9 个误合并对，
`adaptive_0.5 / graph` 也是 9 个；`fixed_70_30 / graph` 为 151 个。
这说明加权在此小样本上减轻了该算法的污染，却没有满足零误合并的选择门槛。
主测试 33 个已评分实例里没有 C 类背影，不能从表中推断无脸召回。

## 背影整块留出

按 E0/E1 的四个含 C 连拍块 3/6/7/9 逐个留出；其余块交替用于
calibration/validation，每个 C 实例恰好被留出一次。下表是每折 validation
选择的完整配置；误合并对统计该折所有已知人物，不限 C。

| 框 | 留出块 | C 个数 | 选中配置 | C 匹配覆盖 | C B-cubed P | 全折误合并对 |
|---|---:|---:|---|---:|---:|---:|
| 人工辅助 | 3 | 5 | adaptive 0.3 / graph | 100% | 1.000 | 0 |
| 人工辅助 | 6 | 3 | fixed / graph | 100% | 1.000 | 0 |
| 人工辅助 | 7 | 5 | fixed / average | 40% | 0.410 | 76 |
| 人工辅助 | 9 | 2 | fixed / DBSCAN | 100% | 1.000 | 0 |
| 自动 | 3 | 5 | fixed / average | 100% | 1.000 | 0 |
| 自动 | 6 | 3 | adaptive 0.3 / average | 100% | 1.000 | 0 |
| 自动 | 7 | 5 | fixed / average | 40% | 0.391 | 93 |
| 自动 | 9 | 2 | face margin 0.1 / average | 0% | 1.000 | 0 |

质量代理在 15 个 C 实例中有 14 个为零；这类节点实际上走 body-only。
块 7 的 average-linkage 将三个已确认身份并入同一预测簇；是否由相似服装主导
还需逐对视觉复核。`adaptive_0.3/0.5`
在该折也得到同样的 40% C 覆盖与 76/93 个全折误合并对。
块 9 自动路由选择 face-only，两个 C 实例均无可用人脸证据，因而漏归组。
这些现象比主测试的零误合并更能说明当前 pipeline 的薄弱处。
15 个互不重复的 C 实例中，所选方案人工辅助匹配 12 个、自动框匹配 10 个，
与 E0 补充留出的 80.0%/66.7% 一致；不能把这种折内配对指标拼成
未声明分布的总体准确率。缺脸时各 adaptive 分数退化为同一个 body cosine，
因此单靠人脸质量加权本来也无法修复块 7 的 body-only 假合并。
固定 `fixed_70_30` 分数仅换聚类器时，块 7 的 graph 将 C 覆盖提到
人工辅助/自动框各 100%，全折误合并对分别为 0/39；average 则为 40%/40%、
76/93 对。图法的合并守卫有价值，但自动框仍远未达到安全门槛。

## 研究判断与下一步实验

1. **E2a：未通过晋级门槛。** 质量低不等于人脸必然错误；从人脸自动转向衣服
   容易引入同服装假阳性。保留质量向量作为证据，不把本轮加权设为默认。
2. **E2b：先补信号，再消融。** 提取时单独记录 detector score、显示方向下的脸像素宽高、
   对齐后 blur、五点残差、脸/人体关联歧义；遮挡和姿态需要独立可复核的观测。
   不能从人工标签给自动节点填值。保留本轮原始 embedding，新增信号版本与哈希。
3. **E2c：校准 `P(same)` 要换独立场次。** 当前仅四个已知身份，同一场次的成对样本
   彼此高度相关，不能靠随机 pair split 宣称低 FPR 或 ECE。按日期/场次隔离训练、
   调参和测试；在 pair 级报告可靠度曲线、误拒绝和 face-unavailable coverage。
4. **E2d：QCFace 是新训练模型/recognizability 表征，不是当前启发式名称。**
   官方仓库列出 IR100/IR18 预训练权重，但权重权利、输出语义、预处理和
   Windows ONNX 还要核验；先在相同低质量桶比较纯编码器和质量输出，
   再进入完整聚类。参见 [QCFace 官方仓库](https://github.com/hpcc-hcmut/QCFace)。

## 聚合 pipeline：建议的实验结构

先在隔离研究目录验证，输入/输出版本化；不修改主应用或已确认身份。

| 阶段 | 输入与产物 | 首选实验及安全门槛 |
|---|---|---|
| 1. 实例证据 | 人/脸框、独立 face/body 向量、质量向量、可用性、时间 | 缺模态显式缺失；保存来源和哈希 |
| 2. 候选边 | face top-k、body top-k 的并集和同场时间邻居 | 先量 candidate recall/size；同图约束仅对确为不同人的实例使用 |
| 3. pair 评分 | 各路 cosine、质量、同图/时间、支持证据 | 清晰且冲突的人脸可否决；弱脸不得被硬 veto；body-only 保持“可能匹配” |
| 4. 高精核心簇 | 高置信互近邻边、完整链接保护和逐次合并审计 | 先守住 core purity/零可见误合并；可保留单例 |
| 5. E3 原型 | core 簇 medoid、质量加权均值、多姿态/服装原型 | 单原型、多原型、trimmed mean 对照；污染簇不得更新原型 |
| 6. E4/E5 局部重排 | core 内 reciprocal 邻居、候选图上下文 | 只改候选顺序；保留 raw cosine；故意注入错边测传播 |
| 7. E6 守卫合并 | 原型、簇间多条独立边、冲突率、margin | 至少两条独立支持、禁止强 face 冲突/同图冲突；记录每次新增错误 pair |
| 8. 人工复核 | 簇、未归组、冲突与低置信边 | 量化拆错簇/合碎簇操作数，确认后才写入全局身份图库 |

“同一照片不可能同一人”在历史导出中有重复身份框，必须先按实例几何与人工语义
验证，不能把同图约束无条件施加到所有历史数据。所有原型只来自训练/当前推理阶段的
高置信 core，不能以 test 真值生成。现场筛图仍要维持按时间在线、不可访问未来图；
后期标注可以完整池重排。匿名 session 簇与跨场全局具名图库保持两套阈值和审核流。

### E3–E6 的执行顺序和晋级条件

- **E3**：先固定 E0 安全图法，只换 mean/medoid/quality-weighted/多原型的
  cluster-to-instance 排序；比较碎簇召回、最大簇召回及污染敏感性。
- **E6 的最小版**：在 E3 后加高精 core + guarded merge，优先检验块 7 的错误合并
  是否被阻断，同时不能把块 3/6/9 的 C 覆盖全部牺牲。
- **E4/E5**：再分别在 core 内做 NFC-like 邻居集中化、稀疏图 rerank；
  每种只增加一个结构变量。报告 2k/5k/10k 节点的候选图时间/峰值内存，
  以及错误边注入后的 overmerge。Pose2ID 官方说明 NFC 是可独立应用的邻居后处理，
  [官方仓库](https://github.com/yuanc3/Pose2ID)；Cheb-GR 当前官方仓库
  [Fast-GCR](https://github.com/Jinxi-Yang-WHU/Fast-GCR) 有代码但没有在本数据上验证。
- **晋级**：优先看假合并的人数/对数、纯簇覆盖、B-cubed、最大人物簇召回和人工修复量；
  只提升 ARI/NMI 或一对一匹配覆盖，不足以晋级。新未见场次是最终验证门槛。

### 后续模型只作为受控替换变量

E1 已表明更快的 LVFace 不保证背影或整个聚类更好；其官方说明源码 MIT、权重仅非商业
研究用途，见 [LVFace 仓库](https://github.com/bytedance/LVFace)。
低分辨率脸可在独立困难桶比较 FaceMoE，但其官方仓库主要提供训练/评估代码和权重目录示例，
需锁定实际权重、前处理及 ONNX 路由后才能做受控对照，见
[FaceMoE 仓库](https://github.com/Kartik-3004/FaceMoE)。
TE-VMamba 需要 selective-scan 原生核，官方示例给 PyTorch checkpoint 评估步骤；
在同一人体 crop 上和 OSNet 比较前，先核验可用权重、Windows 推理、ONNX provider
及真实延迟，见 [TE-VMamba 仓库](https://github.com/YuanHuang0982/TE-VMamba)。
任何权重是否可在产品中分发，都要独立于源码许可证核对。

## 产物与复现

- [脚本](../../model-research/scripts/anonymous_e2.py)、
  [针对缺模态的测试](../../model-research/tests/test_anonymous_e2.py)
- [主评估矩阵](../../model-research/outputs/anonymous-e2-20260923/results.json)、
  [背影折矩阵](../../model-research/outputs/anonymous-e2-20260923/back-holdout.json)、
  [输入/脚本哈希](../../model-research/outputs/anonymous-e2-20260923/provenance.json)

从 `model-research` 执行：

```powershell
.venv/Scripts/python.exe scripts/anonymous_e2.py --root outputs/anonymous-e0-e1-20260923 --output outputs/anonymous-e2-20260923 --back-holdout
.venv/Scripts/python.exe -m pytest -q tests/test_anonymous_e2.py tests/test_anonymous_metrics.py -p no:cacheprovider --basetemp .test-tmp/e2
.venv/Scripts/python.exe -m ruff check scripts/anonymous_e2.py tests/test_anonymous_e2.py
```

本轮测试 9 passed、Ruff 通过。只验证研究脚本与冻结特征评估；没有验证桌面 UI、
真实人工修复次数、跨场次泛化或模型商用权限。
