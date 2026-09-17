# Preview 内容几何设计

状态：首版已实现，2026-09-10。Sony 160×120 JPEG 有界识别；其他布局保守回退。
依据：[Sony HIF 样本研究](../research/2026-09-10-sony-hif-thumbnail-geometry.md)。

## 目标与范围

Sony HIF 的带边 JPEG thumbnail 与 full 使用相同的显示画布和对焦坐标。
保留原 JPEG，不新增 HEVC 解码或完整 metadata 等待。第一版只描述
“预览中的一个矩形覆盖完整主图”的情况；不推广到 RAW 裁幅、透视变换或任意局部预览。

## 公开契约

在 `oxy-domain::PreviewResult` 和对应 TypeScript 类型增加可选字段：

```ts
interface PreviewGeometry {
  displaySize: { width: number; height: number };
  contentRect: { x: number; y: number; width: number; height: number };
}

interface PreviewResult {
  // 原有字段保留：方向校正后的 artifact 栅格尺寸，不是 full 尺寸。
  width: number;
  height: number;
  geometry?: PreviewGeometry;
  // ...其他现有字段
}
```

Rust 使用具名结构体与 `u32` 字段，camelCase serde，geometry 使用
`Option`、default 和 skip_serializing_if。每个结果只增加六个整数及 JSON 字段名。
`MediaResourceDescriptor` 继续只负责资源定位，不承载几何信息。

语义约定：

- `displaySize`：最终主图经过方向校正后的像素尺寸，作为布局、像素缩放和对焦映射的逻辑画布。
- `contentRect`：方向校正后的 artifact 中有效内容区域；左上原点，x 向右、y 向下，
  像素边界坐标，范围为 `[x, x+width) × [y, y+height)`。
- 矩形覆盖完整 `displaySize` 画布；两者比例允许因 thumbnail 整数取整而有微小差异。
  前端明确把该矩形映射至整个画布，不再通过其比例推断裁幅。
- 不再传 rotation。JPEG 的 EXIF 方向由现有解码路径应用，矩形已在旋转后的坐标系中。
- geometry 是完整映射的承诺。只有主图尺寸可信、内容范围和完整覆盖关系均可信时才提供；
  缺失意味着沿用现有行为，不能据此推断没有留边或假造 full 尺寸。
- 不传 confidence 或 Sony 标识给 UI。识别证据与兼容策略属于生产端；不可信结果不发布映射。

真实样本的返回结果（边界已验证；未声明亚像素内容配准）：

```json
{
  "width": 120,
  "height": 160,
  "geometry": {
    "displaySize": { "width": 4672, "height": 7008 },
    "contentRect": { "x": 7, "y": 0, "width": 106, "height": 160 }
  }
}
```

校验：所有尺寸为正；矩形位于 artifact 内；加法使用 checked arithmetic；
已有尺寸上限继续适用。整个 geometry 无效时丢弃，不单独保留半套映射。
整数矩形足够表达第一版像素裁边；未来若需要亚像素配准，应显式升级契约。

## Rust 生产和缓存

1. `formats/heif/quirks/sony.rs` 扩展 `EmbeddedJpeg`，附带可选几何信息。
   基于完整的有界 metadata box 解析 primary item 及其属性关联，避免使用全局最大
   `ispe` 或首个 `irot` 作为可信映射的依据。维持常见 256 KiB、最多 2 MiB 的读取边界。
2. padding 判定留在 Sony 格式层。主图比例只能推导候选范围，仍须验证边缘及布局；
   单个 7px 样本不足以形成所有 Sony 文件的规则。纯黑、真实暗边、未知布局均保守回退。
3. `pipeline/heif/artifact.rs` 将 geometry 与 JPEG 一起发布；不改变 JPEG bytes 或
   `actual_dimensions`，不增加小图重编码。
4. `cache/model.rs::ArtifactPresentation` 增加可选 geometry，通过 artifact 的 variant 元数据持久化。
   所有 publish、lookup、running-result、projection 转换须保留该字段。
   复用现有 JSON manifest，不新增 SQLite 列。缺失字段仍可反序列化为 None。
5. 更新 Sony 快速表示对应的 policy revision，使旧的缺失几何结果不被误当成新版结果。
   无须清空所有媒体缓存；新规则未识别成功的 Sony 仍可返回无 geometry 的快速表示。

同一 source revision 的 full 输出采用相同 displaySize；full 栅格覆盖整个画布，
若携带 geometry，其 contentRect 为全图。geometry 不能提高预览清晰度或满足 native-detail
要求：缓存精度与内存计费仍基于实际像素，绝不能把 displaySize 当作解码尺寸。

## 前端渲染

增加通用内容映射组件/辅助函数，由 Thumbnail、loupe 和 navigator 共用。
设目标画布为 W×H，内容矩形为 (x,y,w,h)，artifact 为 A×B：

```text
scaleX = W / w                  scaleY = H / h
imgWidth  = A * scaleX           imgHeight = B * scaleY
imgLeft   = -x * scaleX          imgTop    = -y * scaleY
```

外层按 displaySize 等比 fit 到可用空间并 overflow:hidden；内层 img 显式设置上述尺寸与偏移，
不再叠加 contain/cover。这里允许校正 106×160 的整数取整差异，不能用于任意畸变图片。
grid/filmstrip 原有的格子裁切属于外一层布局，在内容映射之后执行。
geometry 缺失时保留当前 img 行为。

`naturalWidth/naturalHeight` 继续记录真实栅格，用于浏览器缓存和验证，不能改写成 full 尺寸。
当前 `onImageLoad(size)` 扩展为 `DisplayedPreviewSize`，保留实际 width/height 并附带 geometry。
assetId、source URL/资源身份由现有 `DisplayedImage` 一起保留；loupe 回调按 active.id 隔离。
已有解码完成与资源报告逻辑继续单独处理。

## 状态切换和对焦框

当前 Thumbnail 的 `displayedImage` 只保存 source/resourceId，必须同时保存该表示的 geometry。
新的 projection/result 到达仅更新 pending 表示，不修改仍在显示的图像映射。
pending 图片完成解码后，一次提交 URL、geometry 和可见状态；浏览器内存命中路径遵循同样规则。

loupe 的 thumbnail 状态保留实际栅格与 geometry；full 尺寸回调只在首 tile 绘制、
完整 artifact 加载或内存中的完整表示重新挂载时提交。session 创建不更新显示尺寸。

对于有可信 geometry 的 Sony thumbnail，从首绘起即使用 displaySize；full session
尺寸到达只校验一致性，不改变画布。对焦 metadata 到达可以显示对焦框，但
`mapFocusRegions` 的目标尺寸使用 displaySize，不再传带边 thumbnail 的 naturalSize。
full 首像素/完整 artifact 就绪后的升级沿用该画布，因此只改变清晰度。

无 geometry 路径也不在 session 创建时提前改变对焦映射。首版在 full 首像素绘制时
切换到该表示的尺寸。若可信 primary metadata 与实际 decoder 尺寸冲突，tile canvas 会保持隐藏，
等所有 tiles 成功绘制后再提交画布及尺寸，避免和旧 thumbnail 混合。

RAW 现有相机裁幅推断维持原路径。这套 geometry 暂不描述“预览仅覆盖主图的一部分”；
后续如有真实需求，再增加主图中的目标矩形，而非提前引入任意变换矩阵。

## 实施与验证

建议按一个跨边界改动完成契约、缓存往返、Sony 生产端和共享渲染，避免只接通冷路径。
必要检查：

- Rust：矩形越界/溢出、方向变换、未知布局回退、真实 HIF 样本、缓存序列化及 warm lookup。
- 前端：矩形映射计算、metadata/session/full 的不同到达顺序、资源切换旧结果隔离，
  内存缓存返回与 navigator 一致性。
- 真实图片：横竖拍、多种比例、暗场；对比 thumbnail/full 中相同物体及对焦点的位置。
- 跨边界运行前端和 Rust 全套检查；测量冷 preview 与 warm loupe，保持现有性能预算。
- 调试实例在验收完成后关闭。

首版只识别 160×120 JPEG，按主图比例推导居中的偶数像素内容范围，再逐行验证黑边及内容过渡。
黑边行通道均值需 ≤5、峰值 ≤24，内容边界均值需 ≥12、峰值 ≥32。未通过即无 geometry，
包括无法可靠区分的暗场。3:2、1:1、16:9 有合成测试；真实相机验证目前仅 DSC00449.HIF。

另修正 HEIF scaled decode 的 native-detail 判定：较小的输出不等于主图完整分辨率。
ImageIO 可将目标 512/4096 舍入为 511/4095，显示要求允许 1px 取整误差，不进行额外放大；
full 的 native-detail 要求保持独立。

浏览器回归入口：`scripts/browser/preview-geometry.browser.js`，覆盖 metadata 延迟到达、
内容映射、完整表示替换和对焦框稳定性。支持注入真实方向校正 JPEG。
