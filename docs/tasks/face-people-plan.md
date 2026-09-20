# 人脸识别与人物整理

状态：核心闭环已完成，仍需跨平台发布验证、外部 XMP 实时监听，以及图片旋转后的人工区域对账。
当前架构边界以 [ADR 0011](../adr/0011-first-party-analyzer-host.md) 和
[数据与状态](../architecture/05-data-and-state.md#人脸-analyzer-的运行与持久化边界) 为准。

## 产品闭环

```text
用户下载两个模型
  → 扫描目录并逐张检测、生成 embedding 与裁切
  → 已提交结果立即进入工作区，可确认、纠正或标记非人脸
  → 扫描结束后原子发布聚类与人物候选
  → 新图片产生待确认候选
```

人物和人工决策是用户事实；检测、embedding、裁切、聚类和候选都是可重建缓存。机器结果
不得覆盖人工确认或否定。重新分析后，人工事实按稳定路径和归一化区域重绑定。

## 唯一生产分析栈

- 检测器：SCRFD-10G KPS，默认单次 640×640 推理。
- 特征模型：AdaFace IR-101，输出 512 维归一化 embedding。
- 对齐：五点相似变换，112×112 人脸输入。
- 执行边界：经校验的第一方 analyzer 子进程；应用进程没有推理回退。
- 模型来源：`3rdpart/face-models/managed.json` 是 URL、文件名、大小、SHA-256、归档入口和许可摘要的唯一清单。

模型不随应用打包。用户在人物工作台分别点击下载；界面实时显示字节进度。Host 流式写入
临时文件、校验大小与 SHA-256，再原子安装。两项都有效后才能扫描。模型版本参与 analyzer
fingerprint，变化时只清除机器缓存，不删除人物和人工决策。

“检测远景小脸”是显式质量模式，额外执行重叠分块，默认关闭。默认检测置信度为 0.5。
SCRFD 的测量与取舍见 [性能记录](../research/scrfd-10g-performance-2026-09-20.md)。

## 运行流程

1. Host 按路径 keyset 每页读取 256 个分析目标，并保存 generation/cursor checkpoint。
2. 每张图片通过既有 Full 媒体管线解码为 display-oriented RGB；图片字节不进入 JSON IPC。
3. RGB 经有界二进制帧传给 analyzer 子进程，控制、几何和错误使用 JSON 帧。
4. 检测、embedding、扫描时 JPEG 裁切和每资产 projection 在一个事务中提交。
5. `face-analysis-progress` 节流触发复核列表与统计的增量刷新；用户此时即可操作已提交人脸。
6. 扫描完成后才重建聚类和人物候选，并通过一次 `face-library-updated` 发布完整派生结果。

浏览优先于后台分析。每个目标在工作前等待 foreground gate；取消先协作通知，超时则终止并
回收子进程。提交前再次校验 source revision，过期结果不会落库。失败资产计入已处理数并保留
错误，后续运行可以从 checkpoint 续跑。

## 聚类与匹配

未知脸使用 reciprocal-kNN 局部图，再按相似度执行平均链接合并。同一照片中的不同人脸形成
cannot-link 约束并随 component 传播，避免链式桥接把不同人物合为一组。聚类仅产生建议，绝不
创建人物或人工决定。

已命名人物的 gallery 使用已确认 embedding。匹配结果进入待确认队列；用户确认、纠正、拒绝
候选或标记非人脸后写入耐久事实。阈值可根据本地接受/拒绝样本给出建议，但不会自动改设置。
精确聚类和匹配保留显式规模上限，超限失败而不是静默截断。

## 持久化边界

| 数据 | 权威来源 | 语义 |
| --- | --- | --- |
| observations、embeddings、crops、clusters、candidates、scan/checkpoint | SQLite | 可重建机器缓存 |
| persons、decisions、clarity marks、undo journal、sync state | `people.json` | 不可丢失的本地用户事实 |
| 人物 XMP 命名空间 | 照片 sidecar | 可交换的人物事实投影 |
| custom tags | 用户数据及 SQLite 投影 | 人物分类与照片标签 |

裁切只读取扫描阶段持久化的 JPEG，不在列表滚动时重新解码原图。缺失裁切显示占位并等待重扫，
从而让复核浏览的耗时和内存与 Full 解码彻底解耦。

XMP 只记录稳定 fact/person ID、随机 revision、display-normalized region、人工决定和人物名称；
不记录 observation ID、embedding、裁切、分数或聚类。同步与普通 XMP 编辑共用锁并使用原子写入；
冲突保留双方供用户处理，不以本机时钟决定覆盖。

## 工作台边界

- 模型区只负责状态、下载/取消、实时进度和许可提示。
- 分析区只负责范围、参数、开始/停止和上一轮结果。
- 照片复核区按页读取，支持状态视图和相似分组；扫描期间动态加入新提交照片。
- 人物管理负责创建、改名、分类、合并和移出人脸。
- React Query 失效分为 `analysis`、`incremental`、`people` 三个范围，扫描进度不会反复刷新模型或人物管理查询。

主窗口与独立工作台仅交换当前目录上下文和“定位此人脸”消息。资源仍通过注册路径和
`oxy-media://` 提供，不经 JSON 传输图片数据。

## 验证

常规变更至少运行：

```bash
cargo test -p oxy-faces
cargo test -p oxy-analyzer-host
cargo test -p oxyviewer commands::faces --lib
cargo test -p oxyviewer jobs::faces --lib
pnpm --filter @oxyviewer/desktop check
pnpm test
```

真实模型测试需要把 `OXY_FACE_MODEL_DIR` 指向包含托管模型对的目录。性能结论必须使用 release
构建和代表性真实图片，不能以编译成功代替。

## 剩余发布工作

- 在 macOS、Windows、Linux 的最终安装包中验证 worker 定位、启动、取消和崩溃回收。
- 为外部进程修改人物 XMP 增加实时监听；当前仍依赖索引刷新。
- 定义图片旋转后人工区域的可证明对账规则。
- 在许可与签名体系完成前，不开放第三方 analyzer pack 安装。
