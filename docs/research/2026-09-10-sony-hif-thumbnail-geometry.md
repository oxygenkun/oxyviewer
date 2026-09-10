# Sony HIF thumbnail 与 full 的显示几何

日期：2026-09-10。本轮研究实际样本和现有代码，未修改生产行为。

## 结论

`DSC00449.HIF` 的快速 JPEG 带有实存的黑边。当前路径只纠正方向，
把整个 JPEG 当成图像内容，导致 thumbnail 与 full 的宽高比不同。
loupe 又会在 full 像素可见之前切换几何信息，因此图像与对焦框分阶段跳动。

应先让 thumbnail 的有效内容映射到 full 的同一逻辑画布，再统一对焦坐标。
“尺寸一致”指显示比例、方向和内容位置一致，不需要把 160px JPEG 放大编码成 full 分辨率。

## 本地样本证据

样本：`tests/fixtures/DSC00449.HIF`，9,490,432 bytes。
读取前 256 KiB，按当前 Sony extractor 的 JPEG marker 搜索策略提取候选，
用 Pillow 解码并逐行统计 RGB；同时直接检查提取 JPEG 的画面。

| 项目 | 结果 |
| --- | --- |
| 可解码的小 JPEG | 文件偏移 155648，8294 bytes，160×120 |
| 上方留边 | 原始 JPEG 的 y=0…6，共 7 行 |
| 下方留边 | y=113…119，共 7 行 |
| 有效内容 | y=7…112，160×106 |
| 主图编码尺寸 | 7008×4672；现有 full 路径亦使用该尺寸 |
| `irot` | 3，即顺时针 90° |
| 当前 thumbnail 显示尺寸 | 120×160，比例 0.75 |
| full 显示尺寸 | 4672×7008，比例 2/3 |
| 去留边后的内容尺寸 | 旋转后 106×160，比例 0.6625 |

留边不是严格 RGB=0：JPEG 压缩造成少量非零值。上方各行通道均值不超过
1.43，下方不超过 3.36；y=7 内容开始时均值约 52，y=112 约 81/83/126。
不能依靠逐像素等于黑色判断，也不能把暗场照片任意自动裁切。

160×106 与理论 160×106⅔ 仍有整数取整差异，约 0.625%。简单整数裁边后
继续以 naturalWidth/naturalHeight 定义布局，仍无法获得完全相同的显示比例。
应保留主图的逻辑比例，单独描述 thumbnail 内容矩形及其映射。
这轮没有做 full 下采样与 thumbnail 的逐像素配准，不能声称内容在亚像素层面完全一致。

前缀内还发现 1616×1080、320×212 的 `ispe`，以及主图 tile 尺寸 3520×1600。
仅扫描 `ispe` 不能证明 item 归属；后续应依据 primary item 和属性关联解析。
前 256 KiB 未找到 `clap` 字节标记，不能假设该样本提供可直接使用的 clean aperture。

外部交叉证据：[libheif issue #1406](https://github.com/strukturag/libheif/issues/1406)
报告 Sony ILX-LR1 的 9504×6336 主图带有 1616×1080、320×212、160×120 三档
thumbnail，且存在黑边。它支持此类问题并非本样本独有，但不是 Sony 所有机型的格式保证。

## 当前代码中的两次几何变化

1. `crates/oxy-media/src/formats/heif/quirks/sony.rs`：有界读取前缀，提取 JPEG，
   附加 EXIF 方向；没有内容矩形或裁边处理。`ispe` 取最大面积、`irot` 取首个匹配，
   目前不是基于 primary item 关联的通用几何解析。
2. `crates/oxy-media/src/pipeline/heif/artifact.rs`：将整个 JPEG 发布为 120×160 artifact；
   thumbnail 与 loupe preview 共用它。现有测试明确断言这个尺寸。
3. `apps/desktop/src/lib/loupe.ts`：HEIF 布局尺寸依次优先 full、metadata、preview。
   metadata 到达时，外层布局可能已经从 thumbnail 比例切换为 full 比例。
4. `apps/desktop/src/components/Loupe.tsx`：对焦映射使用的尺寸依次优先 full、preview、
   metadata，与外层布局的优先级不同；存在外框按 full、对焦按带边 thumbnail 计算的窗口。
5. `apps/desktop/src/components/HeifTileCanvas.tsx`：拿到 full session 后立即调用
   `onImageSize`，并不等待 tile 绘制；artifact 路径也在图像显示之前报告尺寸。
   这时父组件切换对焦映射，thumbnail 仍然可见。代码顺序符合用户报告的现象；
   本轮未运行 UI 时间测量，不把报告的几百毫秒当作实测结果。

`mapFocusRegions` 的比例差异分支用于推断相机裁幅或 RAW 扩幅，不能正确解释
Sony JPEG 的 padding。直接让它把 120×160 当内容范围，必然引入不必要的坐标变化。

## 建议的实现顺序

1. 在现有有界 Sony 前缀读取中取得可信主图显示尺寸、方向与 thumbnail 内容矩形，
   随快速表示一起返回。不要为了这些信息等待完整 EXIF、HEVC 解码或递归索引。
2. loupe 从第一张 thumbnail 起使用 full 的逻辑画布。将有效内容矩形映射到该画布；
   单纯改 CSS 宽高、拉伸整张带边 JPEG 或只使用居中 `cover` 都不够精确。
   可选择缓存一次去边的小图，或保留 JPEG 并在渲染层裁边；前者须评估重编码成本，
   后者须明确旋转前后的坐标约定，并让 navigator 使用同一映射。
3. 对焦框使用同一逻辑画布。metadata 可以独立到达，但不能因 full session 提前报告
   尺寸而重新解释尚未切换的 thumbnail 内容。另将“尺寸已知”和“像素已显示”分开。
4. 若改变缓存 JPEG 或几何契约，更新对应缓存版本/身份，避免继续复用旧的带边表示。

7px 是本样本观测值，不能硬编码为所有 Sony HIF 的规则。需继续覆盖横拍、两个方向
竖拍、180°、3:2/16:9/1:1、不同尺寸和机型；无法确认 padding 时应保守回退。
较大的 HEVC thumbnail 也不能直接假定比例完全准确，而且启用它会增加解码等待。

## 后续验收

- 真实样本回归：主图尺寸和方向、有效内容范围、旧缓存失效。
- 不同到达顺序：thumbnail、metadata、session、首个 full 像素；框的位置在清晰化前后稳定。
- 对比边缘物体位置，验证没有把 padding 去除误做成对实际内容的裁幅。
- 测试纯黑、暗场、缺失属性、非 Sony HEIF 的安全回退。
- 保留 256 KiB 常见快路径和 2 MiB 上限；测量冷 preview 与 warm loupe，
  对照 `docs/PERFORMANCE.md` 的 800ms / 150ms 预算。
