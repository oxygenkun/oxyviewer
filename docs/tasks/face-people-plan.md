# 人脸识别与人物整理方案

状态：进行中。分析内核（含分块小脸检测）、缓存/用户数据分层、Host 接线、人物评审界面、
**非 SQLite 的耐久人物存储**（已泛化为 `oxy-userdata` 的通用用户数据机制）、人脸裁切缩略图、
loupe 人脸框 overlay，以及
**merge / 移出人脸 / 撤销**、**重命名/移动/删除后的身份稳定性**均已落地，并附 OpenCV
对照测试、端到端闭环测试、"删库恢复"验收测试、"分块检出小脸"实测，以及**续跑与陈旧
结果**两项发布门槛测试、**基于用户自身确认记录的阈值标定**、**目录范围分析**与
**检测专用分析分辨率**；**独立的人脸识别工作台窗口**与**点击人脸跳转主窗口 loupe**也已完成。
图像旋转后的区域对账仍未完成，因此本文留在 `docs/tasks/`。目标本身（检测 → 聚类 → 簇命名
→ 新图待确认 → 确认/修正）已全部实现并有测试覆盖，剩余项属于加固与体验，不是目标缺口。

本方案实现 [deep-research-report.md](../deep-research-report.md) 的主张：**分析器只产出机器观测，
Host 拥有人物与人工事实，标签与 XMP 只是人物数据的投影**。评审报告是设计依据，本文记录
实际落地范围、已验证证据和剩余工作。

## 1. 目标

用户想要的第一个真实功能闭环：

```text
扫描 → 检测人脸 → 算 embedding → 聚合相似人脸 → 用户给簇命名
     → 新图片识别出人脸 → 进入待确认 → 用户确认正确 / 纠正错误 / 标记非人脸
```

参数由用户控制（检测严格度、匹配严格度、聚类松紧），并且修改某一级参数**只重算该级及其
下游**，不重新解码、不重跑检测。

## 2. 不可协商的不变量

| 不变量 | 落地方式 | 验证 |
| --- | --- | --- |
| 机器观测可重建，用户事实不可丢 | `oxy-library` 的 `face_observations` / `face_embeddings` / `face_candidates` / `face_clusters` 是缓存；`persons` / `face_decisions` / `face_decision_events` 是用户数据 | `faces::tests::a_decision_survives_even_when_its_face_disappears` |
| 换检测器不能让 Alice 消失 | 决策绑定归一化区域；重新分析按 IoU ≥ 0.5 重新绑定到新 observation | `reanalysis_keeps_decisions_and_rebinds_them_by_overlap` |
| 人工否定必须保留 | `FaceDecision::{ConfirmPerson, RejectPerson, NotFace}` 全部落库，绝不"删除机器结果" | `not_face_suppresses_a_redetected_face` |
| 机器不得覆盖人工 | 评审队列按 人工确认 > 人工否定 > 机器候选 > 未知 取值 | `a_candidate_is_pending_until_the_user_answers` |
| 聚类不是人物 | `FaceCluster` 只存在于缓存表，`PersonId` 独立于 `cluster_id` 与标签名 | `clusters_round_trip_per_fingerprint`、`person_rename_does_not_touch_confirmations` |
| 检测置信度 ≠ 身份相似度 | `detection_score` 与 `similarity` 是不同字段、不同阈值 | `FaceAnalyzerSettings` 分为检测/匹配两组参数 |
| 图片字节不进 JSON | 分析器只接受已解码的 `RgbImage`；embedding 以 BLOB 存储 | `oxy-library` 的 `encode_embedding` / `decode_embedding` |

## 3. 已落地

### 3.1 `oxy-domain`：契约

`crates/oxy-domain/src/faces.rs`：

- 几何：`NormalizedRect`、`NormalizedPoint`、`PixelSize`，含 `iou` / `clamp_unit`。
- 机器观测：`FaceObservation`（含 `source_revision` 与 `detector_fingerprint`）、`FaceCandidate`、`FaceCluster`（含 `outlier_observation_ids`）。
- 用户事实：`Person`、`FaceDecision`、`FaceDecisionRecord`。
- 评审：`FaceReviewState`、`FaceReviewItem`、`FaceReviewFilter`、`FaceReviewPage`。
- 参数：`FaceAnalyzerSettings`（检测阈值 / NMS / 最小脸 / 匹配灵敏度 / 兜底阈值 / 聚类阈值）、`FaceMatchSensitivity`。
- 重算级别：`FaceRecomputeScope`，明确"改检测参数才重跑检测"。

### 3.2 `oxy-faces`：纯 Rust ONNX 分析内核

新 workspace crate，不依赖系统 OpenCV、CUDA 或 Python：

| 模块 | 内容 |
| --- | --- |
| `image` | `RgbImage`、letterbox、双线性缩放、NCHW 转换（RGB/BGR 两种通道序） |
| `yunet` | YuNet 检测：stride 解码、`sqrt(cls*obj)` 打分、贪心 NMS、top-k |
| `align` | 5 点最小二乘相似变换（Umeyama）+ 零边界双线性 warp |
| `sface` | SFace 112×112 对齐裁剪 → 128 维 embedding |
| `cluster` | 未知脸单链接聚类、凝聚度、代表成员、**边界成员（outlier）** |
| `matcher` | 人物 gallery 最近邻匹配；`consistent_examples` 表示"有多少确认样本同意" |
| `analyzer` | 组合以上步骤，输出 `FaceObservation` + embedding，指纹与 observation id 均由内容派生 |

模型通过 `3rdpart/face-models` 固定（YuNet 2023mar，MIT；SFace 2021dec，Apache-2.0），
`pnpm faces:prepare` 下载并校验 SHA-256 到 `target/native/face-models`。

**已验证的证据。** `crates/oxy-faces/tests/reference_parity.rs` 用 OpenCV 自身生成的
golden 值对照 Rust 实现（`tests/data/generate_reference.py` 可复现）：

- SFace embedding 余弦 > 0.999；
- 真实人像上检测数量、bbox、5 点关键点、score、embedding 全部对齐；
- 对齐 warp 与 `cv::warpAffine` 的逐像素平均误差 < 3/255。

### 3.3 `oxy-library`：缓存与用户数据

`crates/oxy-library/src/faces.rs` 新增表与 API（`Library::open` 与 `in_memory` 都会建表）：

- 缓存：`face_observations`、`face_embeddings`（BLOB）、`face_candidates`、`face_clusters` / `face_cluster_members`。
- 用户数据：`persons`、`face_decisions`、`face_decision_events`（append-only，可撤销）、`face_settings`。
- 关键 API：`replace_asset_faces`（重新分析并按 IoU 重新绑定决策）、`undecided_face_embeddings`、
  `replace_face_clusters` / `face_clusters`、`replace_face_candidates`、`create_person` /
  `rename_person` / `link_person_tag` / `delete_person`、`record_face_decision` /
  `clear_face_decision`、`face_review_page`、`face_library_stats`、`face_analyzer_settings`。
- 几何与相似度以整数微单位（`× 1_000_000`）存储，遵守本 crate "不出现 REAL 列"的既有约定。

### 3.4 `oxy-fs`：耐久写入

`write_atomic`：同目录唯一临时文件 → `write_all` → `sync_all` → rename → 目录 fsync（Unix）。
这是后续把人物与决策写入非 SQLite 耐久存储的基础；`oxy-fs` 此前没有任何写入辅助函数。

### 3.5 Host 接线（`src-tauri`）

- `jobs/faces.rs`：后台分析队列。单线程、每张图先
  `library.foreground.wait_for_background()`（人脸任务只能被浏览抢占）；每张图都写
  checkpoint（含零人脸的图），因此中断后可续跑；`source_revision` 在解码前读取、写回前
  再校验，中途被替换的图不会写入结果。分析结束后重建聚类与候选。模型缺失时报告
  `Failed` 进度而不是崩溃，浏览不受影响。
- `crates/oxy-media/src/rgb.rs`：`decode_rgb_pixels` 把 Full 等级产物解码成 RGB8，`oxy-faces`
  因此不需要知道格式与缓存布局。分析使用 `RenderLevel::Full`。
- `crates/oxy-library` 新增 `face_analysis_targets` / `face_analysis_targets_for_paths`
  （按 checkpoint 过滤，`force` 可忽略）、`face_analysis_counts`、`face_asset_scans`
   checkpoint 表、`confirmed_face_embeddings`、`unresolved_face_embeddings`、
  `face_clusters_current`。
- 命令：`get_face_capability`、`update_face_analyzer_settings`、`start_face_analysis`、
  `cancel_face_analysis`、`list_persons`、`create_person`、`rename_person`、
  `link_person_tag`、`delete_person`、`decide_face`、`clear_face_decision`、
  `get_face_review_page`、`get_face_clusters`、`get_asset_face_observations`、
  `resolve_face_observation`（人脸 → 所属资产，供工作台跳转）；
  事件 `face-analysis-progress`、`face-library-updated`。
  工作台窗口另有 `open/close/is_face_workbench_window_open`、
  `publish/request_face_workbench_context`、`notify_face_asset_reveal`，
  以及事件 `face-workbench-visibility` / `face-workbench-context` /
  `face-workbench-context-request` / `face-asset-reveal`。
  改检测参数会丢弃已加载的 analyzer 并让 checkpoint 失效；只改匹配/聚类阈值时走
  `refresh_without_detection()`，**不重新检测**。

### 3.6 前端

`components/people/FaceWorkbench.tsx`：**独立窗口**里的人脸识别工作台，由侧边栏底部
与设置并排的图标按钮（同一尺寸，便于以后加入其他同类插件）打开，不再是设置面板里的一个
页签。左侧是启动台（大号"开始分析"
按钮、进度条、目录/已加载/整库三种范围、统计与可折叠参数），右侧是人脸分类区（相似人脸组、
人物、待确认队列三个页签），点任意人脸裁切即把主窗口切到该照片的 Loupe。`types.ts` 与
`lib/api.ts` 补齐全部契约与浏览器演示回退。

待确认队列默认筛的是 `FaceReviewFilter::Unreviewed`（"未处理" = 尚无人工决策的全部人脸），
不是 `Pending`：`Pending` 只包含匹配器给出候选的脸，而候选又必须有已命名人物才会产生，
所以新库第一次分析完 `Pending` 必然为空，用 `Pending` 当默认只会让人以为"什么都没检测到"。
`Pending` / `Unknown` 仍作为更窄的筛选保留。

启动台在进度条下方显示**上一轮的产出**（处理了多少张、检测到多少张人脸、多少张读取失败、
失败原因），因为失败的文件同样计入"已处理"，只看进度条会把一次全军覆没的运行显示成成功。

跨窗口只有两条消息，且都经过 Rust 命令（因此 IPC 契约测试能看见它们）：

- 主窗口 → 工作台：`publish_face_workbench_context`（`face-workbench-context`）发布浏览
  目录、已加载路径与界面语言；工作台挂载时用 `request_face_workbench_context` 主动要一次。
- 工作台 → 主窗口：`notify_face_asset_reveal`（`face-asset-reveal`）发送
  `{observationId, assetId, assetPath}`。观察 id 先经 `resolve_face_observation` 解析成资产，
  已失效的 observation 返回 `None`，界面给出"重新分析"的提示而不是报错。

主窗口收到 reveal 后：定位包含该路径的会话 → 切换当前目录 → 等目录分页把目标资产取回
（后台分页本身会继续拉页）→ `select` 并 `setView("loupe")`。目标被搜索/筛选隐藏时给出
明确提示，不会静默失败。

`components/loupe/FaceOverlay.tsx` 仍然是"就地修正"的最短路径。

### 3.7 端到端证据

`crates/oxy-faces/tests/people_flow.rs` 在真实持久化层上跑完整产品闭环：

1. 检测 → 两张未知人脸；
2. 给第 0 张命名 Alice → 第 1 张仍是未知（命名一张脸不等于命名整张图）；
3. 新图入库 → 匹配只对同一张脸给出候选、且是 `pending` 而不是自动确认；
4. 用户确认 → `face_count` 变 2；
5. 用户纠正错误候选 → 候选被清除并记录为已否定，再次匹配也不会复活；
6. 标记非人脸 → 决策保留，确认计数下降；
7. 同一张脸出现在三张图 → 聚成 2 个簇、无异离群、凝聚度 > 0.95，且聚类不产生人物或决策；
8. 更换检测器（bbox 位移、observation id 变化）→ 人工确认按 IoU 重新绑定，Alice 仍在。

### 3.8 耐久人物存储（`oxy-userdata`）

回应用户数据不变量中最强的一条：**SQLite 可以被删除，人物确认不能因此消失。**

这条不变量后来被抽成了通用机制：`oxy-people` 泛化为 `oxy-userdata`，`PersonStore` 只是
`DocumentStore<T>` 之上的一个人物域。理由与实现见 §3.8.1。

- `crates/oxy-userdata`：`PersonStore` 把 `PersonRecord` 与 `DecisionRecord` 写成一个
  `people.json`，用 `oxy_fs::write_atomic`（临时文件 → fsync → rename → 目录 fsync）提交。
  写入先于可见：持久化失败时变更不会生效，调用方不会把一条保不住的确认投影到缓存。
- **权威方向单向**：`people.json` 是权威，SQLite 的 `persons` / `face_decisions` 是投影。
  `Library::replace_user_data` 全量重建投影，因此缓存不可能持有 store 里没有的记录；
  崩溃在"写完文件、还没投影"之间只会导致下次启动刷新投影。
- 决策记录以 `(asset_path, region)` 为身份、`observation_id` 只是**当前绑定**：这正是
  "删库后重新分析再把确认接回新 observation"能成立的原因。
- 首次运行（文件不存在）会把已有 SQLite 用户数据导出并写入 store，这是唯一一次迁移。
- 每次分析结束后 `PeopleService::sync_bindings()` 把投影算出的新 `observation_id` 写回
  store，避免下次启动把已确认的人脸显示成"未绑定"。
- 损坏的 JSON 与更高版本号都报错而不是当成空 store —— 后者会让下一次投影用"空"覆盖掉
  真实确认。人物这份文档用的是严格加载（`DocumentStore::load`），因为它不是"能启动就行"
  的偏好设置。
- 已知代价：每次变更重写整个文件（`O(人数 + 决策数)` 字节 + fsync）。若将来成为瓶颈，
  升级路径是同一 API 背后的追加日志，而不是改动调用方。

### 3.8.1 泛化：`DocumentStore` 与另外两份用户数据

拆分依据不是"人脸 vs 人物"，而是**可重建 vs 不可重建**。按这条轴看，`app_data_dir` 下
本来就有三份同类文档，而只有人物那份是安全的：

| 文档 | 迁移前 | 迁移后 |
| --- | --- | --- |
| `people.json` | 原子写 + 拒绝更高版本 + 损坏即报错 | 不变（`PersonStore` 走 `DocumentStore::load`） |
| `cache-settings.json` | `fs::write` 截断写；没有 version；损坏时静默用默认值并**在下一次保存时覆盖用户设置** | `DocumentStore::load_recoverable`：原子写、版本信封、损坏文件改名隔离后以默认值启动 |
| `external-apps.json` | 自建临时文件 + flush/sync；`version != 1` 即报错，但被拒绝的文件仍会在下一次保存时被默认值覆盖 | 同一 `DocumentStore`：损坏隔离、来自更新版本或校验不通过的文档保留原样并**拒绝写入** |

`DocumentStore<T>` 只承载四件共有的机制，不承载业务：

1. 版本信封（`UserDocument::version` / `stamp`，拒绝更高版本）；
2. `oxy_fs::write_atomic` 原子替换；
3. **先持久化后发布**（写失败不改变内存文档，调用方不会投影一条保不住的记录）；
4. **绝不覆盖没读懂的文件**：无法解析 → 改名 `*.corrupt`（重名时递增后缀）后从默认值启动；
   来自更新版本 → 保留原文件、本次运行拒绝写入（`reset_in_memory` 只修内存、绝不落盘）。

人物特有的东西仍留在 people 域：决策槽位身份（`same_decision_slot`）、撤销日志语义与
64 条上限、`prune_orphaned_decisions`、按 IoU 重绑定、首次从 SQLite 导出。

**每域一份文件**而不是合成一个 `userdata.json`：损坏的爆炸半径（一个坏 JSON 不该连坐
人物确认）、逐域独立的版本与迁移、写放大（点一次人脸确认不该重写全部设置）。

**这次没有迁移的**：`library_roots` 与 `custom_tags` / `asset_tags` 仍留在 SQLite。标签
已经有 XMP sidecar 作为权威落点（`tag_xmp_sync_queue` + `asset_tag_xmp_state`），SQLite
只是它的投影与查询层；显式 root 属于配置，若要外置应连同"导出/恢复"一起设计，而不是顺手
搬一个文件。`face_settings` 同理留在缓存侧：它是可重建的参数，不是用户事实。

`crates/oxy-userdata/tests/durability.rs` 直接执行验收条件：建立带确认的图库 → 把 store 由
缓存播种 → **物理删除 `oxyviewer.sqlite`** → 用同一个 store 重建新图库 → 断言人物与决策
恢复、并在重新分析后重新绑定到新 observation、`Confirmed` 队列再次显示 Alice；
另有两个测试覆盖"投影始终等于 store"与"已否定的候选在删库后仍然阻止再次推荐"。

### 3.9 评审界面：人脸裁切与 loupe overlay

确认界面的核心要求是"让用户看见他正在确认的那张脸"，因此这一层不是装饰：

- `oxy-media::face_crop_jpeg`：把归一化区域裁成方形（默认外扩 1.5 倍以含头发与下巴），
  越界时向图内收缩而不是补黑边，再缩放并编码为 JPEG。区域是归一化的，所以与
  "预览来自哪一级渲染"无关。
- **裁切取哪一级渲染**：所有支持的格式都请求 `RenderLevel::Full`，与分析取图规则一致。
  用 512 px 有界预览裁一张只占画面 5% 的脸，等于把二三十个真实像素拉成 128 px，整个复核
  面板都是糊的。`Full` 对栅格就是源文件（不写新 artifact），但它**没有做方向校正**，所以必须像分析那样按
  `image_facts.exif_orientation` 旋转一次，否则带方向标签的手机照片会裁到完全错误的位置。
- 扫描时从分析器使用的 Full 像素直接生成 128px JPEG，存入 SQLite 的 `face_crops` 可重建表；
  observation 被替换时外键级联删除旧图。旧扫描缺裁切时，首次请求从 Full 补生成并写回。
- `state/face_crops.rs`：优先读插件裁切缓存，再使用**按已编码字节做内存 LRU**（上限 1024 张），
  每次请求重新注册成 `oxy-media://` 资源交给前端租用。图片字节不经过 JSON IPC。
- `get_face_crops` 异步转到阻塞工作线程；Full RAW/HEIF/TIFF 裁切串行执行以限制峰值内存，
  JPEG/PNG/WebP 可独立完成，不必排在慢速 HIF 后面。
  已失效的 observation 被跳过，不影响同批其余人脸。
- `components/people/FaceCrop.tsx`：每张人脸独立请求，完成一张就显示一张；
  只加载当前页签，持有资源租约、卸载时释放，并**每 10 秒续租一次**。
  注册出来的资源只带一段 publish grace（5 秒）的 UI 租约，而前端的 `retainMediaResource`
  只是本地计数、不会续租；不续租的话，切走再切回页签时渲染的会是 Host 早已丢掉的那批 URL，
  而 `staleTime: Infinity` 又让查询永远不重取，人脸就永久变成灰方块。续租失败（资源已被
  淘汰）时按成员集合**重新注册一次**——重试那个不可变的旧 URL 永远不会成功。
  `FaceOverlay.tsx` 复用同一个 hook，不再自己维护一套租约逻辑（它之前也没有续租）。
- `components/loupe/FaceOverlay.tsx`：在 loupe 图片上直接画人脸框（归一化区域 → 百分比，
  缩放时始终对齐），点击即可 ✓ 确认候选 / ✗ 纠正 / ⊘ 非人脸；未确认的脸可以就地输入
  姓名并确认。这是"修正"最短的路径：不用离开照片。框的颜色按状态区分，未确认框可开关
  （偏好持久化，默认开启）。

`get_asset_face_reviews` 让 loupe 一次拿到"这张图的人脸 + 用户当前的答案"，前端不需要
自己 join observation 与 decision。

### 3.10 分块小脸检测

评审指出"大合影里的小脸会被降采样掉"。现在 `analysis_views` 会在整幅 letterbox 之外
追加重叠分块：

- 长边 ≤ 检测输入的 1.5 倍时不切块（收益不足）；否则切 2×2，长边超过检测输入 4 倍时切 3×3。
  分块之间重叠 15%，跨缝的人脸仍完整落在相邻块内。
- 整幅视图始终保留：单个分块会漏掉跨越多个分块的大脸。
- 所有视图的检测结果都映射回源图归一化坐标，再按 score（同分则按位置，保证确定性）排序、
  做归一化 IoU 合并，最后才分配 `local_index`。排序确定性正是"重复分析不产生重复人脸"的前提。
- 对齐与 embedding 仍在**原图**上做，不用放大后的分块，避免小脸 embedding 质量下降。
- 用户可见开关 `detectSmallFaces`（默认开）。它属于检测级参数：改它会刷新检测指纹、
  使已存观测失效，但绝不重新检测；反过来改匹配/聚类阈值只走 `refresh_without_detection()`。
  `commands::faces::tests::only_detection_parameters_force_re_analysis` 守着这条边界。

**实测**（`reference_parity.rs::tiling_recovers_small_faces_a_single_pass_misses`，把已提交的
人像 fixture 平铺 N×N 造出合影尺寸，期望人脸数 = N²×2）：

| 平铺 | 图像 | 期望 | 整幅单次 | 分块 |
| --- | --- | --- | --- | --- |
| 3×3 | 2400×1410 | 18 | 18 | 18 |
| 4×4 | 3200×1880 | 32 | 24 | **32** |
| 5×5 | 4000×2350 | 50 | 25 | **50** |
| 6×6 | 4800×2820 | 72 | 0 | **72** |

也就是说：长边约 3200 px 起，单次检测开始系统性丢脸；到 4800 px 时干脆一张都检不到，
而分块仍能全部找回。

### 3.11 人物操作：merge / 移出人脸 / 撤销

评审把这三种操作列为发布门槛：`Alice + Alica → merge` 可撤销、把误属 Alice 的三张脸
split 出去不影响其余照片、rename 不触发重算。实现如下：

- **操作在耐久 store 上完成**（`merge_persons` / `remove_faces_from_person` /
  `assign_faces_to_person`），再整体重建 SQLite 投影。用户的一次显式操作付一次
  `O(人数 + 决策数)` 是可以接受的；这与"逐张分析"必须批量处理不同。
- **撤销日志是用户数据，不是缓存**：`PersonOperation` 追加在 `people.json` 里
  （`version` 从 1 升到 2；v1 文件仍可读，v2 文件会被旧构建拒绝而不是静默丢掉日志），
  上限 64 条，是"纠错"而不是"历史"。
- **撤销是精确的**：每个操作都保存它替换掉的状态——merge 保存整个被合并人物的记录与
  被改写的决策原文；"移出人脸"保存被移除的决策；"分配人脸"保存它新增的 observation
  列表与被替换的决策。撤销因此不需要从"已经变了"的投影反推原状。这也顺带修掉了一个
  真实缺陷：只记录被替换决策时，撤销一次"给没有决策的脸分配人物"会什么都不做。
- `same_decision_slot` 用 observation id 作为槽位键；当决策的 observation 已被替换
  （`None`）时退回 `asset_path + region + 决策类型`，所以换检测器之后 merge 依然可撤销。
- 删除人物（`remove_person`）保留 `NotFace` 决策："这不是人脸"说的是区域，不是某个人。
- UI：人物区提供"合并到…"两个下拉与一个**写明内容的**撤销按钮（"撤销合并：Alica（1 张）"），
  已确认人脸在待确认列表里可以直接"移出此人"。
- 改动人物姓名仍然只是改标签：不触发任何重算，`person_id` 是身份。

`crates/oxy-userdata/tests/durability.rs` 覆盖了最强的一条：**merge 之后删掉整个 SQLite，
重新打开仍能撤销**，撤销后 Alice 与 Alica 各自恢复原有的照片数——也就是说撤销日志
确实和确认数据一样活过了缓存删除。

### 3.12 恢复能力与陈旧结果（发布门槛）

评审把这两条列为"人物功能不能发布前必须通过"的门槛。两道保证分别位于分析器与库：

- **恢复**：`face_asset_scans` 对每张图记录 `(source_revision, detector_fingerprint)`，
  零人脸的图也记。`face_analysis_targets` 只返回 checkpoint 不匹配的图，所以任务被杀掉后
  重新开始会**继续**而不是重来，且重复执行不会产生重复观测。
- **陈旧结果**：修订号契约下沉到 `oxy-domain`（`face_source_revision` /
  `face_source_revision_for_path` / `FACE_REVISION_VERSION`），库的 checkpoint 比较与分析器
  的判据共用同一个定义。`FaceAnalyzer::analyze_verified` 在分析结束后重新读取文件修订号，
  不一致就返回 `None`——而不是把属于旧字节的结果写成新观测。Host 拿到 `None` 时保持
  checkpoint 不变，于是下一轮还会再访问这张图。

一个关键实现细节：**队列给出的 `source_revision` 来自索引，可能落后于磁盘**（图在索引之后
被修改）。所以 Host 在解码前用 `face_source_revision_for_path` 自己读一次，分析器再对
"实际解码的那份字节"校验；这条不对称性有测试专门断言。

新增证据（`crates/oxy-faces/tests/people_flow.rs`，用真实模型与真实库）：

- `analysis_resumes_from_checkpoints_without_duplicates`：3 张图 → 只分析 1 张（模拟被杀）→
  队列只剩 2 张且不含已完成的那张 → 跑完 → 队列为空 → 6 个观测且 id 无重复 →
  对同样字节重跑一遍，观测集合**逐字节不变**（内容派生 id 带来的幂等）。
- `a_result_is_discarded_when_the_source_changed_during_analysis`：读到修订号后替换文件 →
  `analyze_verified` 返回 `None` → 库中没有任何观测、checkpoint 未写入、该图仍在队列里 →
  用磁盘当前的修订号再分析则被接受。

### 3.13 重命名 / 移动 / 删除：身份稳定性

评审的门槛写着"文件重命名、目录移动…都不能让已经人工确认的 Alice 消失"。之前不成立：
决策按 `asset_path` 记录，改名就等于把确认留在了旧路径上。标签早在 `execute_file_operation`
里做了状态迁移，人脸数据现在照做，但多一层——**耐久 store 才是权威**：

- `PersonStore::move_decisions` / `copy_decisions` / `remove_decisions`：移动保持
  `observation_id` 绑定（缓存行也会一起移动）；复制产生的是**未绑定**的决策，因为副本
  还没有观测，等它被分析时会按区域重新接上；删除只移除该资产的决策，人物本身保留。
- `Library::move_asset_face_state` / `remove_asset_face_state`：只搬运机器侧缓存
  （`face_observations`、`face_asset_scans`），**不碰** `face_decisions`——用户数据只有一个
  权威来源，避免两处各写一半。
- `PeopleService::move_asset` / `copy_asset` / `remove_asset`：先写耐久 store，再移动机器
  缓存，最后整体重建投影。命令层在 `execute_file_operation` 里与标签迁移并列调用。
- 文件操作**不写撤销日志**：撤销文件操作属于文件服务的职责（roadmap 里的 undo journal），
  不是人物日志的职责。

证据（`crates/oxy-userdata/tests/durability.rs`）：

- `a_rename_moves_the_confirmation_and_it_survives_a_cache_wipe`：改名后新路径**立刻**显示
  已确认（观测也搬过去了），旧路径不留决策；随后删掉整个 SQLite 重新打开，Alice 仍挂在
  新路径上；再次分析后确认自动接上新 observation，用户不必重新确认一次。
- `deleting_an_asset_drops_its_face_data`：删除一张图只清掉它的观测与决策，人物与另一张图
  的照片数不变。
- store 单测另覆盖改名只动目标资产、复制不带绑定、无决策时是空操作。

已知缺口：文件移动与耐久写入不是同一个事务。若进程恰好死在"文件已改名、store 未更新"
之间，决策会停留在旧路径上（**不会丢失**，只是暂时找不到对应图片，重新分析后按区域
接回新路径）。彻底修复需要与文件服务共享的两阶段日志，属于 roadmap 中"文件写入缺少
undo journal"的同一项工作。

### 3.14 阈值标定：用用户自己的数据

评审强调阈值不是通用常数："必须最后拿你自己的真实数据集调 threshold"。参数 UI 因此不再
只有预设，而是能回答"在我的图库上，这条线该画在哪里"。

- **采样**：每次用户回答一个候选（✓ 或 ✗），就把该候选的相似度随决策一起记下来
  （`DecisionRecord.proposed_similarity`）。分数本身与阈值无关——阈值只决定"是否生成候选"
  ——所以这些样本是干净的。SQLite 侧新增 `proposed_similarity_micro` 列（含针对旧库的
  `PRAGMA table_info` + `ALTER TABLE` 迁移），`export_user_data` 会把它读回来，因此"从缓存
  迁移进耐久 store"不会丢掉标定历史。
- **两类样本只取有提案的**：从零命名的人脸没有分数可学（正确）；"这不是人脸"被排除——
  它说的是检测器，不是匹配。
- **算法**（`oxy-faces::recommend_threshold`）：每类至少 3 例才给建议；在相邻观测分数的
  中点里选使 Youden's J（TPR − FPR，以"接受"为正类）最大的那个；J 相同时取离数据点最远
  的候选，避免阈值压在某个样本上。
- **诚实的偏差说明**：低于当前阈值的分数从未成为候选，也就从未被接受或否定，所以样本集
  偏向当前设置之上。建议值是**对当前设置的细化**，不是无条件替换。UI 在两类分布重叠时
  额外提示"这只是当前证据下的最优折中"。
- **UI**：参数区显示"已接受 N 例 / 已否定 M 例"、建议阈值，以及一个"采用建议值"按钮
  （写入 `matchSensitivity: custom` + `matchThreshold`）；样本不足时只显示原因，不给按钮。

测试：算法单测覆盖样本不足、清晰分离、重叠分布（断言必然有 1 例被误分且被误分的是被否定
一侧——漏掉真实人脸才是用户会察觉的错误）、阈值不落在数据点上、非有限值被忽略、
极窄间隔仍可用；Host 单测用真实候选与决策走通"记录分数 → 标定 → 给出建议"，并验证从零
命名的人脸不产生样本。

### 3.15 目录范围分析

"分析当前文件夹"原先只发送浏览器**已加载页面**里的资产路径，所以一个 2000 张照片的目录
只会分析前几百张——对"扫描 → 聚合 → 确认"这条链路是真实的功能缺口。

- `FaceAnalysisRequest` 增加 `root_path` / `directory`，并给出三个构造器
  （`library` / `directory` / `paths`），优先级为 **显式路径 > 目录 > 整个图库**。
  省略作用域时序列化不写 `null`，老前端仍然能发 `{paths, force}`。
- `Library::face_analysis_targets_in_directory` 按 `root_path + parent_path` 过滤，与浏览器的
  非递归语义一致："这个文件夹"就是网格正在看的这一层，但覆盖它的**全部**已索引资产，
  而不是已加载的那些。
- 顺带把三个 target 查询合并为一个私有 helper（`face_analysis_targets_scoped`）：checkpoint
  谓词只存在一处，作用域作为可拼接的 `AND` 片段，参数从 `?4` 起。之前是两个近乎逐字重复的
  查询，再加一个就是三份。
- UI 现在是三个动作：**分析当前文件夹**（目录范围，未打开文件夹时禁用）、
  **只分析已加载的照片**（显式路径）、**分析整个图库**。

关键实现细节（有测试断言）：**索引里存的是规范化路径**，所以调用方必须传会话持有的那套
路径；macOS 上 `tempdir()` 返回 `/var/...` 而索引存的是 `/private/var/...`，两者不相等。
方法文档写明了这一点。

测试：库单测覆盖目录范围返回全部资产（非仅一页）、`limit` 仍可用于分页、不同 root 不会
串入、不存在的目录返回空而不是报错；另有单测确认"显式路径"与全库查询共用同一 checkpoint
谓词。前端单测断言三个按钮各自发出的请求 payload。

### 3.16 检测专用分析分辨率（JPEG 的精度天花板）

`RenderLevel::Preview` 对栅格格式返回的是**投递限幅缩略图**（`THUMBNAIL_EDGE = 512`），
不是原图。于是 24MP 照片进入检测器时已经被缩小十倍以上：一个原图 100px 的人脸只剩 8.5px，
而默认 `min_face_pixels = 24`，**直接被丢弃**——合影在 JPEG 上几乎无法分析。分块检测也救不了，
因为 512px 的源根本不触发分块。

当前分析和裁切对所有格式都通过 `preview_for_app_upgrade` 请求 `RenderLevel::Full`，
关闭临时预览返回。栅格格式（JPEG/PNG/WebP）的 Full
就是源文件本身；RAW/HEIF/TIFF 则通过各自的 Full 产物取得更高细节。
`detectSmallFaces` 只控制分块检测，不改变取图等级。模型固定输入所需的缩放在分析器内另行进行。
检测指纹含 `full-v1`，使旧版 Preview 分析结果在下一次分析时重新计算。

两个必须处理的安全与正确性细节：

- **方向**：栅格源文件尚未显示校正，其他格式的 Full 产物已经校正。分析坐标要与 loupe 绘制的一致，
  所以栅格源文件经 `oxy_media::apply_exif_orientation`（覆盖 8 种 EXIF 方向，
  未知值按"已正向"处理而不是猜）。没有这一步，手机上竖拍的照片人脸框会全部错位。
- **资源成本**：Full 解码可能占用大量内存，仍在后台任务中串行执行，并遵守媒体管道自身的资源预算。

实测（`reference_parity.rs::full_resolution_analysis_keeps_faces_the_bounded_preview_loses`，
4×4 平铺的 32 张人脸、原图 3200px）：**限幅预览只找到 24 张，原图分辨率找到 32 张**。
测试把这个数字写死，附注说明一旦投递限幅或检测器变化就需要重新测量。

### 3.17 跨语言 IPC 契约测试

在此之前，Rust 与 TypeScript 之间的边界没有任何自动化检查：命令在一侧改名、事件在
一侧换字面量，两边都仍能编译、类型检查通过，只有用户点击时才报错。

`apps/desktop/src/lib/ipcContract.test.ts` 读取两侧源码并断言：

1. 每个 `#[tauri::command]` 函数都出现在 `generate_handler!` 列表里；
2. `generate_handler!` 里的每个名字都有对应函数；
3. 前端（含组件，不只 `api.ts`）的每一次 `invoke("…")` 都是已注册命令；
4. 前端每一次 `listen("…")` 都有 Rust 侧 `emit("…")` 或 `_EVENT` 常量与之对应；
5. 没有以模板字符串拼接命令名（那会绕过以上检查）。

它按源码文本解析，所以有一条"提取到的集合必须非空"的哨兵断言——否则正则失配会让所有
检查静默通过。这条哨兵是必要的：我验证过把 `merge_persons` 改成 `merge_people` 后测试
立刻失败并指出该名字，恢复后重新通过。

### 3.18 打包：模型随包分发

在开发环境里 `resolve_model_paths` 会回退到 `target/native/face-models`，所以功能可用；
但**打包后的应用不会带模型**，人物页只会显示"模型未安装"——这是一个只会在安装后暴露的
功能缺口。

- `3rdpart/face-models/prepare.mjs` 现在把两个 ONNX 与各自的**上游许可证原文**（按
  `source.json` 的 SHA-256 校验）以及一份 `NOTICE.md`（列出文件名、版本、许可证、哈希）
  复制到 `apps/desktop/src-tauri/resources/face-models/`。
- `tauri.bundle.json` 把两个模型映射到包内 `face-models/`，把许可证与 NOTICE 映射到
  `licenses/face-models/`。
- `apps/desktop/scripts/tauri.mjs` 在 `build`/`bundle` 前调用 `prepareFaceModels()`，
  与既有的 FFmpeg / libheif 步骤并列。
- `resources/face-models/` 进 `.gitignore`：它是下载产物，不是源码（约 39 MB）。
- 运行时读取 `resource_dir()/face-models`，与映射目标一致；这条"目录名 + 文件名"的契约
  现在有测试守着（`a_model_directory_needs_both_files`、
  `the_bundled_layout_is_the_one_the_packager_produces`），改一处而忘了另一处会立刻失败，
  而不是只在安装包里静默失效。

## 4. 剩余工作（按依赖顺序）

### 4.1 待补充的验收门槛

| 门槛 | 现状 |
| --- | --- |
| 缓存正确性：中途替换原图，旧 `source_revision` 结果不得写入 | **已通过**，见 §3.12 |
| 恢复能力：任意批次杀进程，重启从 checkpoint 继续且不产生重复 | **已通过**，见 §3.12（进程级 kill 未做，验证的是决定恢复语义的 checkpoint 与幂等性） |
| 浏览性能：分析器崩溃/未安装/模型损坏时浏览行为不变 | 模型缺失路径已实现并有前端测试；真实 release 对照待做 |
| 身份稳定性：重命名/移动/标签改名/换模型都不丢 Alice | **已通过**（改名/移动/删除 + 删库后仍在），见 §3.13；换模型与换检测器见 §3.7 |
| 人物操作：merge/split 可撤销 | **已通过**，见 §3.11 与 `durability.rs` 的删库后撤销测试 |
| 持久化：删除全部 SQLite 后人物关系可恢复 | **已通过**，见 §3.8 与 `crates/oxy-userdata/tests/durability.rs` |
| 模型可替换：换 detector/embedder 后 UI、Person UUID、标签、确认数据与查询 descriptor 都不用迁移 | 指纹分层与重绑定已覆盖；第二组模型端到端待验证 |
| 小脸召回：合影中的远景人脸不得被降采样丢掉 | 已通过，见 §3.10 的实测表 |
| 阈值标定：参数来自用户真实图库而非公有 benchmark | 机制与建议算法已通过测试（§3.14）；真实分布仍需要用户实际使用后积累 |

### 4.2 已知限制

- 分析始终使用 Full。打开"识别远处的小脸"后，大图最多检测 10 个视图
  （1 整幅 + 3×3）；关闭只减少分块模型计算，不改变 Full 解码成本。
- 分析按原图分辨率解码，尚未缓存"分析用"派生图；重复分析同一张图会重复解码。
- 文件移动与耐久写入不是同一事务（见 §3.13 末段）。
- 图像被旋转或编辑后，已存区域坐标不再对应像素；目前只有"换检测器"的重绑定，没有几何
  变换后的区域对账。
- 人脸框使用归一化区域直接映射到 loupe 显示区域，未复用对焦框那套 content-rect 映射；
  带 padding 的 RAW/HIF 预览可能有极小偏移。
- 聚类是 `O(n²)` 精确比较，上限 `MAX_EXACT_FACES = 20_000`。超过时应缩小范围（只聚新增未知脸），
  之后再加 ANN 索引。
- 匹配决策分用"最近确认样本"而非 top-k 均值：新人物往往只有一两个确认样本，均值会把
  正确候选稀释掉。`consistent_examples` 暴露"有多少样本同意"，供 UI 排序与展示。
- 匹配预设仍是 SFace 公有 benchmark 的经验值，作为"还没有样本时"的起点；积累若干确认后
  §3.14 的建议值才代表这个图库。
- 标定样本偏向当前阈值之上，且只有"被提案过"的人脸参与；从零命名的人脸不贡献样本。

## 5. 维护入口

| 改动 | 需要同时更新 |
| --- | --- |
| 模型版本 | `3rdpart/face-models/source.json`、`3rdpart/face-models/README.md`、`pnpm faces:prepare` 后重跑 `generate_reference.py` |
| 检测/对齐/嵌入实现 | `crates/oxy-faces/tests/reference_parity.rs` 的容差说明与 golden |
| 缓存表结构 | `docs/architecture/05-data-and-state.md` 的事实来源表 |
| 耐久文档的 schema 版本 | `crates/oxy-userdata/src/document.rs` 的机制 + 各域的 `CURRENT_VERSION`（`people.rs` 的 `STORE_VERSION`、`state/cache.rs` 的 `CONFIG_VERSION`、`state/external_apps.rs` 的 `SETTINGS_VERSION`）；升版本必须同时给出迁移与对旧构建的拒绝语义 |
| 新命令 | `commands.rs`、`lib.rs` 的 `generate_handler!`、`apps/desktop/src/lib/api.ts`、`types.ts` |
| 裁切尺寸/边距/质量 | `state/face_crops.rs` 的常量，并升级 `oxy-library/src/faces.rs` 的 `FACE_CROP_POLICY_VERSION` |
| 分析分辨率/分块上限 | `jobs/faces.rs` 的 Full 请求、`oxy-faces/src/analyzer.rs` 的检测指纹与分块规则；改动后需重跑 §3.16 的实测并更新数字 |
| EXIF 方向处理 | `oxy_media::apply_exif_orientation`（覆盖 8 种方向，有单测） |
