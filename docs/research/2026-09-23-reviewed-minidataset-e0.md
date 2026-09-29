# dataset01 整图确认子集：元数据补全与 E0 pilot

用途口径更新：本次历史 pilot 尚未区分现场筛图与后期标注。后续实验按
[两类用途规范](../../model-research/IDENTITY_USE_CASES.md)分别评估；下列历史混合指标
不直接代表任一用途的验收结果。原始快照和实验数值保持不变。

2026-09-23，使用 dataset01 当时已完成整图确认的 143 张照片建立独立快照，执行
ArcFace、AdaFace、OSNet、固定融合与既有质量门控的检索/聚类对照。未改主应用，
未修改原始 HIF 或人工标注 JSON，未下载新模型。

## 元数据补全

按转换 manifest 将 551 个同名 HIF 的可写元数据复制进 dataset01 JPG：EXIF 拍摄时间、
毫秒、时区、相机、镜头、曝光、评分、XMP 和 Sony MakerNotes。ExifTool 13.59，零警告。
原 JPG 已逐张备份，完整源标签已导出为 JSON。

全部 551 张 JPEG 的非 APP/COM 编码段均与备份一致，包括量化表、Huffman 表、帧头和
压缩扫描数据；补全没有重新编码或改变图像像素。JPG 已在 HIF 转换时转正，因此
Orientation=1，EXIF 宽高使用 JPG 实际尺寸（524 张竖图需要交换原始宽高）。

不能把 HEIF 容器属性原样解释为 JPEG 属性。ExifTool 调整了色度位置、MakerNotes 偏移，
省略原编码的 CompressedBitsPerPixel，并对 MaxApertureValue 有约 2.79796→2.8 的表示
舍入；50 个文件的 XMPToolkit 标记变化。所有差异及源原值均留档。

本地证据：[元数据报告](../../model-research/outputs/dataset01-metadata-20260923/report.md)、
[逐文件审计](../../model-research/outputs/dataset01-metadata-20260923/report.json)、
[编码段校验](../../model-research/outputs/dataset01-metadata-20260923/jpeg-structure-verification.json)。
历史 conversion manifest 的 output_bytes 仍描述最初转换结果，本次哈希以元数据报告为准。

## 冻结数据与划分

- 143 张照片；167 条人物记录中 1 条已删除，166 个有效实例。
- 165 个已知身份：糕糕 65、沙 50、晗歌 37、十七 13；unknown 1；阿夸 0。
- 已确认 face 区域 146、body 区域 157；1 个实例没有区域。
- 人脸质量：clear 109、occluded 27、blur 10；姿态：front 94、profile 51、back 1。
- 无重复身份框、缺失图片/标注文件等数据审计问题。

使用完整 551 张 JPG 的 EXIF 时间（含毫秒和 +08:00 时区）分组，不先丢掉未确认照片。
相邻间隔超过 5 秒才建立新组，整组轮换进入 calibration、validation、test。
全量形成 29 组，当前快照覆盖 11 组；所有组均未跨 split。
这是实验性的连续拍摄分组规则，不是相机提供的 burst ID，也不是跨场隔离。

| 划分 | 照片 | 已知身份实例 | unknown | 覆盖组数 |
| --- | ---: | ---: | ---: | ---: |
| calibration / 检索图库 | 71 | 76 | 0 | 5 |
| validation | 40 | 52 | 1 | 3 |
| test | 32 | 37 | 0 | 3 |

测试集身份分布是晗歌 19、糕糕 13、十七 4、沙 1。因此“沙”的测试结果和困难质量桶
不能用于稳定的统计结论。划分不使用身份标签，未为改善分数重新分配样本。

本地快照及 SHA256：[snapshot.json](../../model-research/outputs/minidataset01-20260923/dataset/snapshot.json)。
快照固定复用，不跟随之后的标注修改或新增确认。

## 提取与评估口径

JPEG 长边缩至 1800；SCRFD960 检测与已确认人脸框按 IoU≥0.5 做一对一匹配，
从五点对齐脸提取 ArcFace/AdaFace；OSNet 使用已确认人体框。未匹配人脸不补猜，
缺特征不删除实例。unknown 保留为聚类节点，但不作为共同身份计分。

146 个已标人脸中，141 个匹配成功，另有 5 个检测框未匹配；后者不能直接称为误检。
ArcFace/AdaFace 特征覆盖 141/166，OSNet 覆盖 157/166。抽查 5 个未匹配脸的裁剪，
4 个有明显手臂遮挡，另一个需要进一步区分检测与几何匹配失败，未因此修改真值。

检索以 calibration 的 76 个已知身份实例为固定图库，validation/test 为查询，排除同图
候选。mAP 分母保留缺特征正例；无查询证据记 0。本轮 37 个 test 查询均有同身份图库参考。
这测的是人物实例检索，不是完整自动检测端到端产品，也没有进行开放集拒识阈值验证。

## 检索结果

| 方法 | validation mAP | test mAP | test Top-1 | test Top-5 |
| --- | ---: | ---: | ---: | ---: |
| ArcFace | 0.6332 | 0.7817 | 94.59% | 97.30% |
| AdaFace | 0.6365 | 0.7765 | 91.89% | 94.59% |
| OSNet | 0.6363 | 0.5946 | 70.27% | 91.89% |
| 固定 AdaFace/body 0.7/0.3 | 0.7923 | 0.8119 | 89.19% | 91.89% |
| 既有质量门控 | **0.8352** | **0.8903** | **94.59%** | **97.30%** |

质量门控由 validation 选出。这里的质量是检测分、脸尺寸与模糊度的既有启发式，
不是人工质量标签直接进入分数，也不是训练完成的质量校准器。

本次可观察到：

- 人体能补回无脸实例，但单独 OSNet 不够稳定：test 11 个 Top-1 错误均来自晗歌。
- 固定融合会损害部分本来正确的脸证据；质量门控提升整体排序，但 Top-1 与 ArcFace 相同。
- 质量门控剩下两个错误均为十七：`DSC00918`（遮挡/侧脸）与 `DSC00921`（模糊/侧脸），
  Top-1 错误图库标签均为糕糕。前者启发式质量仍达 0.6097，提示质量分并不等于可识别性。
- test 的 32 个 clear 查询，质量门控 Top-1=100%；blur 仅 2 个、occluded 仅 2 个、
  缺 face 区域仅 1 个，不能宣称已验证困难场景泛化。

## 聚类结果与选择纪律

每个方法只在 calibration 扫描阈值，validation 按 pair F1、precision 选方法。
验证集选出 **fixed_fusion_graph，threshold=0.40**：

| 指标 | validation | test | full（仅描述） |
| --- | ---: | ---: | ---: |
| pair precision | 1.0000 | 1.0000 | 1.0000 |
| pair recall | 0.7109 | 0.6863 | 0.5898 |
| pair F1 | 0.8310 | 0.8140 | 0.7420 |
| ARI | 0.7611 | 0.7297 | 0.6683 |
| 误合并对 | 0 | 0 | 0 |

该图法较保守，测试集十七的 4 个实例全部分散，晗歌分成 3 片。
同人的过度拆散仍是关键问题，不能只报告零误合并。

对照中 quality_gated DBSCAN/平均链接和 fixed_fusion 平均链接 test pair F1 达 0.9634，
但它们未由 validation 选中。这里完整公开结果，**不看过 test 后更换“获胜方法”**。
OSNet 图法 test 有 105 个误合并对；该图法沿用人脸质量建核心，不能当成独立 body-quality
模型。全部 15 个组合见 [原始指标报告](../../model-research/outputs/minidataset01-20260923/e0/report.md)。

## 可复现性与验证

- 冷缓存提取 508.68 秒：模型加载 1.43、解码 92.09、检测 37.51、人脸特征 263.14、
  人体特征 113.87 秒。CPU，线程设为 4；这不是 release 桌面性能基准。
- 热缓存 143/143 命中，缓存载入阶段 0.125 秒；不含 Python 导入、快照/模型哈希检查，
  不得与冷推理声称同类速度提升。
- `features.npy`、`metadata.json`、`report.json`、`clusters.json`、`splits.json`
  冷/热 SHA256 全部相同。单次全方法评估约 1.58 秒。
- Python 单元测试 **44 passed**；新增脚本与测试的 Ruff 检查通过。
- 全目录 Ruff 仍有既有 `check_review_status_ui.py:67,101` 两个 E501，未改无关文件。
- 模型/代码/依赖版本/快照哈希保存在
  [provenance.json](../../model-research/outputs/minidataset01-20260923/e0/provenance.json)。

## 当前结论和下一轮边界

这次完成了可复现的 **E0 pilot**，不是完整 E0–E9。质量门控可作为这批数据的检索候选，
聚类仍需减少同人拆散；暂没有证据要求先换 backbone。

下一轮优先研究质量校准和多样化身份原型，并审计少数身份/侧脸/遮挡错误。已有 test 已被
观察，后续方法修改在本快照上的提升只能视作探索性结果；确认结论需要冻结新的未见数据。
完整 E0 还缺自动人体检测对照、5 秒分组阈值敏感性、峰值内存和完整错误审核。
跨场、换装及阿夸身份尚未验证。

运行方式见 [BENCHMARK.md](../../model-research/BENCHMARK.md#整图确认-minidatasete0-pilot)。
