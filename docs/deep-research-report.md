# 图片管理器插件系统与“人物”功能首发方案架构评审

## 总体判断

**整体方向是对的，而且比“先造一个通用插件 SDK”稳健得多。**我会保留你最核心的几个判断：不要允许插件直接进入主 WebView/React 运行环境；把能力拆成声明式整理、后台分析器、Host 渲染三类窄接口；插件只产出数据，Host 掌握文件系统、调度、持久化和 UI；自动分析结果与人工确认数据严格区分。

但如果目标是**第一个真实功能就做到“人脸检测 → 相似脸聚合 → 人物命名 → 后续自动推荐 → 用户确认/纠错/非人脸 → 长期可迁移”**，现在的方案还需要几处结构性调整。最重要的不是再扩一个 UI descriptor，而是把“缓存”“人脸观测”“人物身份”“人工事实”拆干净。

我的结论可以概括成：

| 你现在的判断 | 评审结论 | 建议 |
|---|---|---|
| 三条窄接缝，而不是万能插件 API | **强烈赞成** | 保留 |
| Tier 1 外部进程承载模型/native/GPU | **赞成，但不能称为 sandbox** | v1 先做受信任的一方/签名 pack；第三方插件以后再加真正 OS 隔离 |
| `resource_projections` 承载派生数据 | **适合 per-asset 结果，不适合整个人物关系模型** | 检测结果放 projection；embedding、person、candidate、assignment 建规范化插件表 |
| `projection_kind='faces:detector-v3'` | **不建议** | semantic kind 与 producer/model version 分离 |
| 人名/人工确认属于用户数据 | **完全正确，而且是整个方案的核心不变量** | 不只存“人名”，确认/纠错/not-face/merge/split 都是用户数据 |
| 人物直接复用现有标签系统 | **可以复用，但标签不能成为人物模型的唯一真相源** | `Person UUID ↔ tag_id` 关联；标签只是查询/互操作投影 |
| XMP `subject` 保存人物确认 | **不够表达多脸照片** | 至少保存 region + person UUID；优先 MWG Region 或项目 namespace |
| 聚类结果属于缓存 | **完全正确** | 永远不要把 cluster ID 当人物 ID |
| 新照片自动进对应人物 | **应改为 candidate/pending，而不是自动归属** | v1 全部人工确认；以后再加高阈值 auto-accept |
| iframe 作为 D 类逃生口 | **方向可以，但当前安全假设不够严** | 延后；真做时优先独立零 capability WebView，而不是主 WebView 内 iframe |
| Sony FocusRegion → 通用 overlay | **非常好的首个 UI 接缝** | 人脸框直接复用这一模式 |

这里特别指出一点：我没有从当前已连接的 GitHub 仓库中定位到你描述的那套 `oxy-*` 私有代码，因此下文对 `ADR 0001/0007`、`resource_projections`、`JobRegistry`、`focusArea.ts` 等仓库事实，是**按你提供的仓库现状作为前提进行架构评审**，不是声称我已经独立验证了这些文件。

从外部技术生态看，你选择“外部 analyzer sidecar”也很自然。Tauri 官方明确支持将外部 executable 作为 sidecar 打包，并通过 stdin/stdout 控制子进程；这很适合 Python、自包含 native runner、ONNX 模型和平台专属 GPU runtime。citeturn12search3

不过，**“子进程”本身不等于“沙箱”**。这将是我首先修改你文档措辞和安全模型的地方。

## 最需要先改的几个架构点

### Tier 1 应叫“隔离进程”，暂时不要叫“沙箱”

你现在写的是：

> 插件不碰文件系统，Host 负责路径与授权，然后把 path / handoff 给插件。

这可以成为**协议规定**，但对一个普通 native child process 来说，并不是安全边界。除非你在 Windows/macOS/Linux 分别施加操作系统级 sandbox，否则一个恶意或被攻陷的 executable 仍然拥有启动它的用户所拥有的大量系统权限。

签名和 SHA-256 解决的是：

> “这是不是我们认可的那一份二进制？”

而不是：

> “这份二进制即使恶意，也无法读取用户其他文件。”

所以建议把 Tier 1 重新定义为：

```text
Tier 1: Managed Trusted Analyzer Process
        受 Host 生命周期、协议、资源和数据流管理的外部进程
        v1 仅加载内置/官方签名 pack
```

真正开放第三方 AnalyzerPack 时，再增加 platform sandbox/profile。换句话说：

```text
signed ≠ sandboxed
out-of-process ≠ least privilege
no filesystem in API ≠ no filesystem capability
```

这也是为什么你现在“只读插件”的限制仍然很有意义，但它首先限制的是**Host RPC 的写操作能力**；它不能自动限制 native executable 自己调用 OS API。

### `projection_kind` 不要混入模型版本

我不推荐：

```text
faces:detector-v3
faces.v3.person
```

作为长期接口。

这里混在一起的是两种完全不同的东西：

```text
语义是什么？            face.detection
谁算的？                builtin.faces
哪个程序版本？          1.4.2
哪个模型？              yunet-2026may
模型文件具体是哪一份？  sha256:...
怎么预处理的？          align-v2
结果 schema 是什么？    schema-v1
```

一旦把 `v3` 放进 `projection_kind`、UI predicate、RPC 字符串，你下一次换 detector/embedding 就会出现：

```text
UI descriptor
       ↓
faces.v3.person
       ↓
数据库 kind
       ↓
模型版本
```

四层被一个字符串绑死。

更好的结构是：

```jsonc
{
  "projectionKind": "face.detection",
  "schemaVersion": 1,
  "producer": {
    "pluginId": "builtin.faces",
    "pluginVersion": "1.4.2"
  },
  "model": {
    "id": "yunet",
    "version": "2026may",
    "sha256": "..."
  },
  "preprocessVersion": "detect-input-v1"
}
```

UI 永远问：

```text
face.detection
face.person
face.pending
```

而不是问：

```text
faces.v3.person
```

这意味着**升级模型不会成为 UI breaking change**。

### 一个 `Analyzer.version()` 不足以做缓存失效

人脸功能至少存在五级计算：

```text
asset
  ↓
face detection
  ↓
alignment / crop
  ↓
embedding
  ↓
unknown clustering
  ↓
known-person matching
```

它们的失效条件不同。

例如，用户只是把“人物匹配敏感度”从严格改成宽松，不应该重新解码 100,000 张图，更不应该重新跑 detector。换 embedding 模型，也没有理由重新检测 bbox。

推荐让每一级拥有自己的 fingerprint：

```text
DetectionFingerprint
  = detector_model_sha
  + detector_preprocess_version
  + inference_parameters_affecting_detection

EmbeddingFingerprint
  = embedder_model_sha
  + alignment_version
  + embedding_preprocess_version

ClusteringFingerprint
  = embedding_fingerprint
  + clustering_algorithm
  + clustering_parameters

MatcherFingerprint
  = embedding_fingerprint
  + matcher_version
  + threshold_policy
```

于是设置变化的成本非常清楚：

| 用户修改 | 需要重算 |
|---|---|
| 人脸检测阈值/最小脸尺寸，且影响 detector 输出 | detection → downstream |
| embedding 模型 | embedding → clustering/matching |
| 聚类松紧程度 | **只重新聚类未知脸** |
| 人物匹配敏感度 | **只重算 candidate** |
| 人名 | **什么模型都不跑** |
| 标签颜色 | **什么模型都不跑** |

这会直接决定 10 万照片库以后是不是还能用。

### `source_revision` 很重要，但写入时必须原子检查

你的“迟到结果拒绝”思想正确。但仅仅在 output 上携带：

```text
path + source_revision
```

并不能天然阻止 stale write。

正确条件实际上是：

> analyzer 开始分析时看到 revision R；当结果写回数据库时，Host 必须在同一事务/原子条件中确认**当前权威资产 revision 仍然等于 R**。

也就是说，逻辑应该是：

```text
analyze(asset=A, source_revision=42)

            ...图片被编辑/替换...

current asset revision = 43

result(A, source_revision=42)
             ↓
       HOST REJECTS
```

而不是先无条件 overwrite projection，再依靠读端发现它旧了。

这也是我建议“Host 是唯一 projection writer”的另一个理由。

### 长期实体不要以 path 为 identity

`path` 非常适合做 I/O locator，却不适合成为：

```text
Person assignment
Face annotation
User decision
```

的长期身份。

照片改名：

```text
IMG_1234.JPG
      ↓
Tokyo-2026-001.JPG
```

显然不应该导致 Alice 消失。

如果你的 domain 已经有稳定的 resource/asset ID，就应让所有人物关系以它为主键：

```text
asset_id     ← identity
path         ← current locator
```

如果确实没有，这反而是人脸功能上线前值得补上的 domain 能力。

## 人脸模块不要建成“特殊标签插件”，而应该建成“人物实体 + 标签投影”

你提出“复用现在标签系统，加前缀标识人脸模块特殊标签”，**作为 UI 和搜索集成非常合适；作为底层数据模型则不够。**

原因很直接。

一张照片里有：

```text
Alice
Bob
Charlie
```

一个资产级标签可以表达：

```text
这张照片包含 Alice
```

但无法表达：

```text
左边 bbox 是 Alice
右边 bbox 原本建议 Bob，但用户说错了
中间这个 detector 输出其实不是人脸
```

因此建议至少区分下面这些实体。

```text
Asset
 └─ FaceObservation          机器发现的、可重建
      ├─ FaceEmbedding       机器算的、可重建
      ├─ FaceCandidate       机器建议的、可重建
      │
      └─ FaceAnnotation      人工事实、不可随缓存删除
             │
             └─ Person      稳定人物实体
                    │
                    └─ Tag  复用现有标签系统
```

### 人物必须拥有独立 UUID

不要：

```text
person_id = "Alice"
person_id = tag name
person_id = cluster id
```

应该：

```rust
struct Person {
    person_id: Uuid,
    display_name: String,
    linked_tag_id: Option<TagId>,
}
```

于是“爱丽丝”改名成“Alice Zhang”只是：

```text
display_name:
  爱丽丝 → Alice Zhang
```

不会破坏任何 face assignment。

标签可以是：

```text
system:person/<person_uuid>
```

或者标签记录具有：

```text
kind = Person
owner = builtin.faces
```

但**稳定关系要绑定 `person_id`/`tag_id`，不能靠标签字符串前缀解析身份。**

### Cluster 绝不能成为人物实体

这是首版最容易埋的坑。

例如第一次运行：

```text
cluster-0021
  face-1
  face-7
  face-12
```

用户点：

```text
“这是 Alice”
```

正确含义不是：

```text
cluster-0021.name = Alice
```

而是：

```text
create Person(Alice)

confirm face-1  → Alice
confirm face-7  → Alice
confirm face-12 → Alice
```

因为下次：

```text
换 embedding
改 clustering threshold
多导入 2 万照片
```

`cluster-0021` 完全可能不存在了。

密度聚类方法如 HDBSCAN 的优点之一就是能够形成层次聚类并把一部分样本保留为 noise，而不是强迫每个人脸都归入某个组，这与“未知脸初始整理”的问题形状比较吻合。citeturn12search0 但无论最后采用 HDBSCAN、DBSCAN 还是 kNN 图，**cluster 都只能是建议结果，不是用户数据**。

### 人工决策需要显式成为一等公民

你描述的 UX 实际上至少需要下面三类用户动作：

```text
✓ 是这个人
✗ 不是这个人
⊘ 这不是人脸
```

千万不要把后两种实现成“删掉机器结果”。

否则：

```text
今天：
模型：这是 Bob
用户：不是 Bob
程序：删除 candidate

明天重新扫描：
模型：这是 Bob
```

用户会永远重复劳动。

应保留 decision：

```rust
enum FaceDecision {
    ConfirmPerson { person_id: PersonId },
    RejectPerson  { person_id: PersonId },
    NotFace,
}
```

并且定义优先级：

```text
人工确认
   >
人工否定
   >
机器推荐
```

**机器永远不能覆盖人工事实。**

这里还可以进一步做得很好：保存 append-only 或可撤销的用户操作记录。

```text
FaceDecisionEvent
  event_id
  region_id
  action
  old_value
  new_value
  timestamp
```

那么“误操作”“人物 merge”“人物 split”都能支持 undo，而不用设计一堆特殊恢复逻辑。

### 推荐的数据表边界

我会让 `resource_projections` 继续保存“这张图检测到了什么”：

```json
{
  "faces": [
    {
      "observationId": "...",
      "bbox": [0.12, 0.16, 0.19, 0.27],
      "landmarks": [...],
      "score": 0.97
    }
  ]
}
```

但是跨资产关系进入规范化表：

```sql
face_observations (
    observation_id,
    asset_id,
    source_revision,
    detector_fingerprint,
    local_index,
    x, y, w, h,
    detection_score,
    PRIMARY KEY (observation_id)
);

face_embeddings (
    observation_id,
    embedder_fingerprint,
    vector_blob,
    quality,
    PRIMARY KEY (observation_id, embedder_fingerprint)
);

persons (
    person_id,
    display_name,
    linked_tag_id,
    created_at
);

face_candidates (
    observation_id,
    person_id,
    matcher_fingerprint,
    similarity,
    state              -- pending / rejected
);

face_annotations (
    annotation_id,
    asset_id,
    x, y, w, h,        -- normalized durable region
    person_id,         -- nullable for not_face
    kind,              -- confirmed_person / not_face
    user_revision
);

face_decision_events (
    event_id,
    annotation_id,
    decision_kind,
    payload_json,
    created_at
);
```

其中还有一个微妙但重要的设计：**人工事实最好绑定一个 model-independent 的 durable region，而不是永远绑定某个 detector 生成的 `observation_id`。**

因为：

```text
YuNet v1 bbox = [100, 100, 180, 180]
新 detector bbox = [97, 103, 183, 176]
```

这是同一张脸。

模型升级后可以用：

```text
region IoU
+
embedding similarity
```

把新 observation 与旧的人工 annotation 做 reconciliation；**不能因为 detector ID 变化就把人工确认 Alice 丢掉。**

## 首版人物功能应该这样运行

你描述的产品流程非常合理，但我建议明确拆成“两种算法模式”：

```text
第一次建库：
无标签的人脸 → 无监督聚类 → 用户创建/确认 Person

以后新增照片：
新脸 → 对已确认 Person 做检索 → pending candidate → 用户确认
```

**不要每新增一张照片就重新做一次全局聚类。**

### 初次扫描

Host 枚举：

```text
Asset A
  ↓
Host render/decode
  ↓
Detector
  ↓
face bbox + landmarks
  ↓
alignment
  ↓
Embedder
  ↓
embedding
```

检测和识别应该明确分开。OpenCV 当前官方接口本身也是这个结构：`FaceDetectorYN` 负责检测，`FaceRecognizerSF` 负责对齐/特征提取/比较；YuNet 与 SFace 是官方示例组合。citeturn11search2

之后只对：

```text
没有人工身份
不是 not-face
```

的人脸做初始聚类：

```text
unknown embeddings
       ↓
HDBSCAN / kNN graph
       ↓
ClusterSuggestion[]
       ↓
review UI
```

页面可以是：

```text
┌─────────────────────────────────────┐
│ 可能是同一个人 · 47 张              │
│                                     │
│ [face] [face] [face] [face] ...    │
│                                     │
│ 姓名 [ Alice              ]         │
│                                     │
│ [全部确认] [逐个检查] [拆分]        │
└─────────────────────────────────────┘
```

这里我会特意增加“疑似离群项”：

```text
高置信成员   → 默认选中
边界成员     → 待检查
```

不要因为一个聚类中有 42/47 张 Alice，就无条件把剩余 5 张也永久写成 Alice。

### 人物被确认之后

假设用户确认了一批 Alice：

```text
Alice
 ├─ embedding A
 ├─ embedding B
 ├─ embedding C
 └─ embedding D
```

后续新照片不再首先问：

> “它属于哪个 cluster？”

而是问：

> “它离哪些已知 Person 的确认样本最近？”

这是一个典型的向量相似度检索问题。Faiss 的定位就是对 dense vectors 做 similarity search/indexing，也包含 clustering primitives；当人脸规模达到较大数量后，这类 ANN 索引适合作为可重建的插件缓存。citeturn12search1

首版甚至不必急着加入 ANN。如果只有：

```text
20 人
每人保留 10 个代表 embedding
```

不过几百次 cosine/dot-product，直接 exact search 更简单。等人脸数增长后，再给 AnalyzerPack 加 Faiss/HNSW 类索引，不需要改变 Person domain。

而且**不要只保存 Alice 的一个平均 centroid**。真实图库常有：

```text
正脸
侧脸
童年
成年
眼镜
强逆光
```

首版更稳健的做法是每个 Person 维护若干确认过、质量较好的 gallery examples，然后：

```text
new face
    ↓
top-k confirmed samples
    ↓
aggregate by person
```

不需要一开始就做在线训练。

### 新照片进入 pending，而不是直接“认定”

推荐状态机：

```text
                  ┌─── no good match ──→ unknown
new face ─ match ─┤
                  └── candidate ───────→ pending
                                            │
                  ┌─────────────────────────┼──────────────────┐
                  ↓                         ↓                  ↓
             ✓ Alice                   ✗ Alice             ⊘ not face
                  │                         │                  │
             confirmed              rejected match       suppressed
```

特别重要的是把：

```text
detection confidence
```

和：

```text
identity similarity
```

完全分开。

例如：

```text
detectorScore = 0.99
```

只表示模型很确信“这是个人脸”。

完全不能解释成：

```text
99% 是 Alice
```

相似度 threshold 也不应硬编码成一个跨模型永久有效的常量。OpenCV 官方 SFace 示例对不同 benchmark 给出的 cosine threshold 本身就有差异，这说明阈值属于具体模型、度量与评估条件，而不是通用的人脸识别常数。citeturn11search2

UI 最好把原始参数翻译成：

```text
人物匹配：
○ 严格
● 平衡
○ 宽松

高级：
similarity threshold = ...
```

而不是要求普通用户理解 cosine distance。

### 以后再考虑自动确认

首版建议：

```text
所有已知人物的新脸
        ↓
pending
        ↓
用户确认
```

先收集你自己真实图库里的：

```text
accepted score distribution
rejected score distribution
```

以后才能合理定义：

```text
candidate_threshold
auto_accept_threshold
```

形成：

```text
score < candidate
       → unknown

candidate ≤ score < auto_accept
       → pending

score ≥ auto_accept
       → 可选自动接受
```

即使最终提供自动接受，我仍建议默认关闭。

这会比“模型一上来自己给照片写 Alice 标签”安全得多，因为误归属的成本明显高于多一次 review。

## 用户数据与 XMP：这里要比现方案再严格一层

你已经抓到了最重要的原则：

> 自动聚类是 cache；用户确认的人物身份是 user data。

我赞成，而且建议扩大定义：

| 数据 | 属性 |
|---|---|
| bbox detection | 可重建 cache |
| landmarks | 可重建 cache |
| embedding | 可重建 cache |
| ANN index | 可重建 cache |
| cluster | 可重建 cache |
| machine candidate | 可重建 cache |
| 人物名字 | **用户数据** |
| 人工确认 face → person | **用户数据** |
| 人工拒绝 Alice | **用户数据** |
| not-face | **用户数据** |
| merge/split | **用户数据** |
| 人物 rename | **用户数据** |

### 仅写 `xmp:subject` 不足够

XMP 本身是可扩展的元数据体系，Exiv2 也明确提供 XMP 的读写支持。citeturn12search2

但是：

```text
xmp:subject = Alice
```

只能表示：

> 这张照片和 Alice 有关。

无法表示：

> `[0.18,0.23,0.14,0.20]` 这个脸框是 Alice。

对于一张多人合照，这是实质区别。

因此建议：

```text
XMP / durable sidecar
  ├─ person definitions / stable IDs
  └─ regions
       ├─ normalized geometry
       ├─ person UUID
       ├─ display name (可选冗余)
       └─ decision provenance
```

这里可以评估 Metadata Working Group 的 Region Schema；Exiv2 的 XMP metadata 体系包含相关 region namespace 支持，因此你无需把所有东西都退化成关键词。citeturn12search2

如果出于项目控制力考虑，自己的 namespace 更简单，也可以：

```text
oxy:FaceRegions
  oxy:Region
    oxy:PersonId="uuid..."
    oxy:X="..."
    oxy:Y="..."
    oxy:W="..."
    oxy:H="..."
```

然后额外将：

```text
Alice
```

投影到现有 tag / `xmp:subject`。

这样第三方软件即使不理解你的 region namespace，至少仍然知道这张照片有 “Alice”；而 Oxy 自己可以精确恢复哪张脸是 Alice。

### 标签是 projection，不是人物真相

推荐：

```text
Person Alice
   │
   ├── stable person UUID
   │
   ├── face-region confirmations
   │
   └── linked existing tag
            ↓
        xmp:subject
```

而不是：

```text
tag "faces:Alice"
       ↓
猜出 Person Alice
```

“前缀”可以保留，适合用于：

```text
系统标签隔离
搜索
样式
是否默认显示
同步策略
```

比如：

```text
kind = system.person
namespace = builtin.faces
```

会比把字符串名字真的编码成：

```text
face:Alice
```

更稳。

### 这也暴露了你首版真正的阻塞项

如果你仓库确实坚持：

> SQLite 可以被完全删除并重建。

那么**在 metadata write / project durable storage 尚未有安全写入边界之前，人物确认功能还不能算完成。**

模型先跑起来不难。

真正可能造成产品级数据事故的是：

```text
用户花 5 小时确认人物
      ↓
SQLite 被清理
      ↓
全部消失
```

所以你写的：

> “在写授权边界落地之前，插件必须只读”

是对的，但它还意味着：

> **可以先做 face detection / embedding / clustering demo；不能把“人物确认”作为正式可用功能发布，直到确认结果拥有非缓存的 durable write path。**

sidecar 写入也不应该让 face analyzer 自己做，而应该永远走：

```text
Face UI
   ↓
Host domain command
   ↓
metadata mutation service
   ↓
atomic sidecar write
   ↓
refresh SQLite cache
```

这样插件体系完全不需要获得文件写权限。

## 插件契约和 UI 方案大体正确，但接口应再收紧

### `Analyzer` 应该从“函数调用”升级为“有协议的作业”

现在：

```rust
fn analyze(
    &self,
    batch: &[AnalyzeInput],
    cancel: &CancellationToken
) -> Vec<AnalyzeOutput>;
```

更像 library API，而你的 Tier 1 实际上是：

```text
长时间
外部进程
可能 crash
需要 progress
需要 backpressure
需要 timeout
需要 restart
需要部分结果
```

建议 domain 层抽象接近：

```rust
pub trait Analyzer: Send + Sync {
    fn descriptor(&self) -> &AnalyzerDescriptor;

    async fn analyze(
        &self,
        request: AnalyzeBatch,
        sink: &mut dyn AnalyzeOutputSink,
        cancel: &CancellationToken,
    ) -> Result<AnalyzeSummary, AnalyzerError>;
}
```

底层协议则显式握手：

```jsonc
{"type":"hello","protocolVersion":1,"pluginId":"builtin.faces"}

{"type":"analyze","requestId":"r1",
 "asset":{
   "assetId":"...",
   "sourceRevision":"..."
 },
 "input":{
   "kind":"host-render",
   "handoff":"..."
 }}

{"type":"result","requestId":"r1",
 "assetId":"...",
 "sourceRevision":"...",
 "projection":{...}}

{"type":"progress","requestId":"r1","completed":17,"total":32}
```

Host 至少要验证：

```text
protocol version
requestId
assetId
sourceRevision
projection kind
bbox range
array length
blob size
JSON line max size
NaN/Infinity
超时
输出数量上限
```

并且只允许有限个 batch in-flight，形成 backpressure。

### 图片字节不要走 JSON 这个判断正确

这不只是风格问题。Tauri 的前后端命令默认对普通值使用 IPC 序列化；官方甚至专门为较大的 binary response 提供原始 array-buffer response 路径以避免 JSON 序列化成本。citeturn14view0

对于 face analyzer 更自然的是：

```text
JSONL = control plane

image/render
embedding batch
large result
       = data plane
```

可以是：

```text
受控工作目录
memory mapping
file descriptor/handle
binary blob file
```

然后 JSON 只传：

```json
{"blobRef":"...", "length":2048}
```

顺便，512 维 `float32` embedding 自身就是：

```text
512 × 4 = 2048 bytes
```

如果 20 万张人脸：

```text
2048 × 200000
≈ 390.6 MiB
```

仅原始向量就接近 400 MiB，所以**不要把 embedding 当 JSON float array 长期塞进 `result_json`**。BLOB 或插件自己的二进制索引更合适。

### A/B/C 类 UI 的判断我会原样保留

人脸首版所需 UI 其实全落在：

```text
A Collection
B Overlay
C Parameter Form
```

确实没有理由让插件运行 React。

尤其是 B：

```text
Face analyzer
       ↓
normalized geometry
       ↓
OverlayDescriptor
       ↓
Host Loupe
```

这正是一个很好的稳定边界。

我建议 generic overlay 甚至不要出现 `face`：

```ts
type OverlayDescriptor = {
  id: string
  kind: "rect" | "point" | "polyline"
  bounds?: NormalizedRect
  points?: NormalizedPoint[]
  label?: string
  confidence?: number
  interaction?: {
    selectable?: boolean
    actionId?: string
  }
}
```

那么：

```text
Sony focus
Face boxes
OCR boxes
Object detection
Crop suggestions
```

全都可以用它。

### D 类 iframe 先不要做

这个结论比你的原方案还要更强：

**人脸首版甚至整个第一版插件系统，都没有必要做 D。**

而且将来做时，我建议重新审视“主 WebView 内 iframe = 没有 Tauri API”这一安全假设。

Tauri 官方当前 Capability 文档明确指出，capability 可以按 window/WebView 授权；但也特别警告：**Linux 和 Android 上，Tauri 无法区分 embedded iframe 发起的请求和包含它的 window 本身。**同一文档还指出，经普通 `invoke_handler` 注册的 app commands 默认允许所有 app window/WebView 使用，除非你进一步通过 app manifest 对 commands 做限制。citeturn14view0

因此将来 D 类更稳的形状是：

```text
main privileged WebView
        │
        │ host-controlled IPC
        ▼
separate plugin WebView/window
        │
        └── ZERO Tauri capability
```

而不是：

```text
privileged main WebView
        └── untrusted iframe
```

RPC 再做：

```text
random per-instance channel token
schema validation
method allow-list
rate limit
subscription limit
asset capability
```

所以你写的：

> 不给 iframe `invoke`

概念上正确，但不能只通过“插件代码没有 import Tauri API”来保证；应该由 Host/Tauri capability 层真正 deny。Tauri 自己也强调 capabilities 是用来细粒度限制哪些 window/WebView 能接触哪些系统能力的。citeturn14view0

## 首版模型与执行环境的推荐

### 我会优先把 YuNet + SFace 做成参考实现

不是因为它们一定是“2026 最强模型”，而是因为你的**第一目标应该是验证完整产品闭环和插件契约，而不是赢 benchmark**。

OpenCV Zoo 当前 YuNet 模型目录明确把它描述为轻量人脸 detector，并给出 WIDER Face 评测；当前默认模型还提供 dynamic input shape。该模型目录明确采用 MIT License。fileciteturn11file0L1-L6

SFace 官方模型目录提供 MobileFaceNet/SFace 识别模型、5-landmark alignment 和量化版本，并明确声明该目录文件采用 Apache-2.0。fileciteturn8file0L2-L2

所以用它们实现：

```text
detect
→ 5 landmarks
→ align
→ embedding
→ cosine similarity
```

能够非常快地验证你的：

```text
AnalyzerPack
projection
embedding storage
clustering
overlay
review UI
person/tag/XMP
```

整个链路。

我不会把 OpenCV Zoo benchmark 数字解释成你的真实相册准确率，因为家庭相册有：

```text
年龄跨度
遮挡
小脸
侧脸
旧照片翻拍
极端光照
```

必须最后拿你自己的真实数据集调 threshold。

### InsightFace/ArcFace 可以支持，但要把“算法”和“模型授权”分开

ArcFace 本身是很成熟的人脸 embedding 方法族；如果将来要提高识别能力，非常值得作为另一 AnalyzerPack。真正需要警惕的是**预训练权重的许可证**。

InsightFace 官方 README 目前明确区分：项目代码采用 MIT；但其训练数据以及使用这些数据训练的模型受非商业研究用途限制，而且对于如 `buffalo_l` 这样的公开 face-recognition model，官方现在明确要求联系获取 licensing。fileciteturn6file0L1-L2

因此不要因为看到：

```text
InsightFace repo = MIT
```

就推导：

```text
buffalo_l weights = 可以随商业桌面应用自由打包
```

这两个命题并不等价。

我会把 AnalyzerPack manifest 里的模型信息做成一等字段：

```jsonc
{
  "models": [
    {
      "id": "face-recognizer",
      "sha256": "...",
      "license": "Apache-2.0",
      "notice": "NOTICE.txt"
    }
  ]
}
```

也就是说你现有 ExifTool pack 里的：

```text
license
notice
SHA-256
```

设计非常值得推广到 AI model。

### 推理 runtime 建议留在 pack 内，不进入 Host domain

ONNX Runtime 当前本身支持多平台以及 CPU/GPU/NPU execution providers，包括 CUDA、TensorRT、OpenVINO、DirectML 等。citeturn11search1

这恰好说明：

```text
Host
  不应该知道 CUDA
  不应该知道 TensorRT
  不应该知道 OpenVINO

AnalyzerPack
  自己选择 execution provider
```

Host 只看：

```json
{
  "computeDevices": ["auto", "cpu", "gpu"]
}
```

pack 自己将：

```text
gpu
```

映射成：

```text
Windows / NVIDIA → ...
Intel → ...
macOS → ...
```

这样以后换 runtime 不会污染 `oxy-domain`。

首个开发版本甚至可以：

```text
Python + OpenCV
```

快速打通协议；

但发行版我更倾向：

```text
self-contained native sidecar
+
ONNX Runtime / OpenCV DNN
```

而不是依赖用户系统 Python。Tauri 的 sidecar 机制正是为这种随应用打包的外部 executable 设计的。citeturn12search3

## 我建议最终落成的首版架构

综合下来，我会把你的原方案收敛成下面这个结构：

```text
                    ┌──────────────────────────────┐
                    │            Host              │
                    │                              │
Filesystem ───────► │ Asset authorization          │
                    │ Render service               │
                    │ JobRegistry                  │
                    │ ProjectionStore              │
                    │ Metadata mutation / XMP      │
                    │ Person domain                │
                    └───────────┬──────────────────┘
                                │
                   narrow analyzer protocol
                                │
                                ▼
                    ┌──────────────────────────────┐
                    │     Face Analyzer Pack       │
                    │                              │
                    │ Detector                     │
                    │ Alignment                    │
                    │ Embedder                     │
                    │ Clusterer                    │
                    │ Candidate matcher            │
                    │ ANN index                    │
                    └───────────┬──────────────────┘
                                │
                          derived data only
                                │
              ┌─────────────────┴─────────────────┐
              ▼                                   ▼
 resource_projections                      plugin cache tables
 face.detection                            embeddings
                                           clusters
                                           candidates
                                           ANN
              │
              └─────────────────┬─────────────────┘
                                ▼
                      Host Person Domain
                                │
                 ┌──────────────┴──────────────┐
                 ▼                             ▼
            existing tags                durable regions
                                             / XMP
```

这里最大的架构变化是：

> **“人脸 AI”是插件；“人物身份”最好不是插件私有概念，而是 Host 能理解的用户领域数据。**

否则未来第二个人脸插件出现时：

```text
Plugin A person Alice
Plugin B person Alice
```

会变成两套互不兼容的人物世界。

更合理的是：

```text
Analyzer A
Analyzer B
    ↓
Face observations / candidates
    ↓
Host Person Alice
```

模型是可换的。

Alice 不是。

## 推荐的实施顺序和验收标准

你原来的实施顺序总体正确，但既然**第一个目标就是人物管理**，我会稍微重排：

| 阶段 | 做什么 | 暂时不做什么 |
|---|---|---|
| 架构契约 | ADR；AnalyzerPack；stable asset ID；projection fingerprint；用户数据/缓存边界 | iframe |
| Analyzer 骨架 | fake hash analyzer；crash/cancel/resume/stale-write 测试 | AI |
| Face detection | YuNet；bbox/landmark；通用 overlay | 人物 |
| Face embedding | SFace；binary embedding store；相似度查询 | 自动认人 |
| 初始聚类 | unknown-only cluster review | 全局永久 cluster |
| Person domain | Person UUID；tag link；rename/merge/split | 把 cluster 当 Person |
| Durable confirmation | region → Person；XMP/project durable store；not-face/negative decisions | 只存在 SQLite |
| 增量匹配 | 新图 → candidate → pending → confirm/reject | 默认 auto-accept |
| 扩展性能 | ANN；GPU provider；model packs | 通用插件 UI |
| 最后再评估 | D 类独立 WebView/RPC | 主 WebView 插件注入 |

其中我会把以下测试列为“人物功能不能发布前必须通过”的门槛：

**缓存正确性。** 分析进行到一半时替换原图，旧 `source_revision` 的结果不能写入；更换 detector 只能使 detection/downstream 失效；更换 clustering 参数不能重新跑 inference。

**恢复能力。** 在 10 万资产 job 的任意批次杀掉 analyzer，重启后从已提交 checkpoint 继续；重复执行不得产生重复 face/person assignment。

**浏览性能。** 打开 folder、分页、loupe、visible thumbnail 路径在 AnalyzerPack crash、未安装、模型损坏时行为完全不变。人脸后台任务只能被浏览工作抢占，不能反过来抢占浏览。

**身份稳定性。** 文件重命名、目录移动、标签改名、embedding 模型升级、detector 升级，都不能让已经人工确认的 Alice 消失。

**人工事实优先。** 用户说“不是 Alice”后，同一 matcher 不得不断重新推荐 Alice；用户标记 not-face 后，同一 detector 不得重新制造 review 项；机器结果无论多高分都不得覆盖人工确认。

**人物操作。** `Alice + Alica → merge` 可撤销；把错误属于 Alice 的三张脸 split 出去，不影响 Alice 其余照片；Person rename 不触发 AI 重算。

**持久化。** 删除全部 SQLite cache 后重新打开图库，人工确认的人物关系能够从 sidecar/项目用户数据重新恢复；如果做不到，这项功能就还没有满足你自己的“SQLite 是可删除缓存”不变量。

**模型可替换。** YuNet/SFace 换成另一组 detector/embedder 时，UI、Person UUID、tag、人工确认数据和查询 descriptor 都不用迁移；只需要让机器派生缓存自然失效。这一点才是真正证明你的插件系统设计成功。

最终我会把你的设计从：

> “插件产出 projection，人物通过插件 tag 表达”

调整为：

> **“Analyzer 插件产出机器观测；Host 拥有 Person 与人工事实；标签/XMP 是 Person 的持久化与查询投影。”**

这一个变化会让整个系统的寿命长很多。

就你现在最想先做的人脸功能而言，最小而正确的产品闭环应该是：

```text
扫描
 ↓
检测人脸
 ↓
算 embedding
 ↓
聚类未知人脸
 ↓
用户创建 Alice
 ↓
逐脸/批量确认
 ↓
Person UUID + linked tag + durable region 写入
 ↓
新照片导入
 ↓
找到 Alice 候选
 ↓
pending
 ↓
用户：
   ✓ 正确
   ✗ 不是 Alice
   ⊘ 不是人脸
 ↓
更新人工事实
 ↓
以后机器模型随便换，Alice 仍然是 Alice
```

这会比先实现一个“足够通用”的插件 API 更值得优先投入：**把这一条链完整跑通之后，你的 Analyzer、projection、overlay、collection descriptor、参数 schema、job/resume、用户数据持久化几乎全都经过了真实需求验证；后面的重复照片、模糊照片、智能相册等插件反而会变成这个架构的简单子集。**