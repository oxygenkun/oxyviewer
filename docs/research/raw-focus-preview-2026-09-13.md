# CR3 / RW2 对焦坐标与 RAW 中间预览

## 解析与显示修正

CR3 原先只进入通用 QuickTime 元数据路径，未读取 Canon 的
`moov / uuid(85c0b687820f11e08111f4ce462b6a48) / CMT1..4`。现在分别按
IFD0、ExifIFD、Canon MakerNotes 和 GPS IFD 解析；不扫描 mdat，拒绝越界 box，
单个 CMT payload 限制为 16 MiB。

Canon AFInfo / AFInfo2 / AFInfo3 的归一化使用 AFImageWidth/Height，保留有符号坐标，
EOS 的 Y 轴向上，PowerShot 的 Y 轴向下，最后应用 EXIF 方向。仅绘制有效范围内的
AFPointsInFocus；不把所有候选点或仅选中的点当作已对焦区域。

RW2 从 IFD0 `0x002e` 指向的 JPEG 读取 EXIF/MakerNotes，补充主 IFD 缺少的字段。
修正 Panasonic 标签表：`0x0048` 是 FlashCurtain，`0x004D` 才是 AFPointPosition，
后者包含两个 rational64u，表示 0～1 的位置；保留小数精度并拒绝无效值。
相机未记录框尺寸时，前端继续使用估计框。

Panasonic 的 AFPointPosition 缺失或无效时，回退读取 `0x004e FaceDetInfo`：
按字节序解析数量和最多五个完整的人脸中心/宽高记录，忽略截断和越界区域。
坐标使用 320 宽、相机画面比例的坐标系，映射至相机尺寸后统一应用 EXIF 旋转。
不使用 EXIF 缩略图的外框尺寸，它可能包含黑边。有效 AF 坐标优先于人脸回退。
该区别只存在于 Panasonic 读取层，输出仍是原有 FocusInfo，通用管线、前端和
对焦框样式不作区分。人脸检测记录并不单独证明合焦，但可作为相机保存区域的补充。

标签依据：[Canon](https://exiftool.org/TagNames/Canon.html)、
[Panasonic 原始定义](https://github.com/exiftool/exiftool/blob/master/lib/Image/ExifTool/Panasonic.pm)。

## RAW preview / full 合同

- Thumbnail 保持 512 上界和整目录浏览器保留。
- Preview 通过专用 artifact target 证明已选择最大内嵌 JPEG；即使该 JPEG 小于
  4096，也满足 preview，而不会因为尺寸不足继续显影。
- Full 首次阶段直接准备最大 JPEG。足够覆盖源图长短边各 90% 时返回 Satisfied；
  不足、或显式重新显影时，先发布该 JPEG 为 Interim，再由原有可取消升级阶段显影。
- Loupe 记录当前已解码图片自己的 renderLevel，在最终 descriptor 到达但浏览器仍在
  解码时，继续显示大 Interim，避免重新退回 folder thumbnail。
- RAW preview/full 的 projection policy 已更新，最大 JPEG 的已有有效 artifact 仍可复用。

## 真实夹具验证

使用 Windows Release/WebView2、独立数据/缓存/profile。照片与私有路径不入库。
Canon EOS R6m2 `HI8A7823.CR3` 来自用户指定目录，Panasonic DC-S5M2X
`PANA4954.RW2` 来自已有本地性能夹具。没有清除 OS 文件缓存。

CR3 原始 AF 中心 `(2127,-187)`，AF 画布 6000×4000，方向 8。
最终画布 4000×6000 中为 `(2187,873)`，区域 234×234；ExifTool 与原生输出一致。
Release 截图确认对焦框位于眼部。

该 RW2 的 AFPointPosition 为 `4294967295/1024`、`4294967295/1024`，
ExifTool 同样报告无效 AF 坐标，最初仅支持 AF 的版本在真实页面没有对焦框。
本次补充发现它同样保存一个 Face1Position `(178,83,10,10)`，现在可使用人脸回退。
有效 RW2 AF 坐标的精度、旋转和无效值拒绝另有合成测试；这些人脸样本不能被描述为
已验证真实有效 AF 位置。

补充样本 `PANA4999.RW2`：AFPointPosition / AFAreaSize 无效，但保存一个
Face1Position `(213,76,13,13)`，EXIF 缩略图为 160×106。最初按缩略图尺寸映射的
Release 实测显示普通绿色实线框，位于画面中间人物的人脸上，最终解码图为 4008×6008。
PANA4954 也经原生测试及页面验证输出一个补充框。修正以下黑边问题后，PANA4999
统一按 320 宽的画面坐标映射，方向 8 对应中心 `(1425,2006)`、区域 244×244。

`PANA5004.JPG` / `PANA5004.RW2` 均记录 `(230,101,16,16)`、6000×4000、方向 8，
但 JPG 的 EXIF 缩略图为 160×120（上下带黑边），RW2 为 160×106。
直接使用缩略图外框会让 JPG 的旋转后 X 坐标缩至 1683；RW2 为 1906。
现在统一按画面宽度 320 等比缩放，不计黑边，两者都输出 4000×6000 画布上的
中心 `(1894,1687)`、区域 300×300，通用显示管线和样式不变。
最新 Release 在独立缓存中打开这对照片，截图确认两者绿色框均落在人脸上；
实际显示图均为 4000×6000，DOM 均为 left 43.6%、top 25.6167%、width 7.5%、
height 5%。配对真实文件测试、缩略图有无黑边/缺失的坐标一致性测试、workspace
全量测试、fmt、Clippy 和 Release 构建通过，测试实例已关闭。

三次独立冷应用缓存的原生 full 管线（本地副本）：

| 样本 | 中位时间 | 最大时间 | JPEG 字节数 | 结果 |
|---|---:|---:|---:|---|
| CR3 | 45.23 ms | 45.72 ms | 1,738,566 | Embedded / Satisfied / 4000×6000 |
| RW2 | 61.02 ms | 65.28 ms | 6,056,236 | Embedded / Satisfied / 4000×6000 |

两者都正确取得最大相机 JPEG，没有进入 WIC/LibRaw。RW2 编码字节约为 CR3 的 3.48 倍。
这组数据只覆盖两张照片，不能代表所有 RW2/CR3 或 NAS 冷盘延迟。

最终 Release 每样本三次，新进程、冷应用缓存，两张照片的本地目录：

| 阶段 | CR3 中位 | RW2 中位 |
|---|---:|---:|
| 选中到 full 解码就绪 | 214.6 ms | 310.8 ms |
| Resource Timing duration | 17.2 ms | 48.0 ms |
| load 之后等待 img.decode | 97.1 ms | 142.8 ms |

这里的 full 就绪沿用 `image:loaded` 口径，不等于精确 GPU present。
CR3 三次最大 351.2 ms，RW2 三次最大 315.2 ms，样本量小，不视为稳定分位数。
本次 RW2 差距主要出现在 JPEG 传输和浏览器解码，未发现最大 JPEG 获取失败。

强制显影实测：CR3 进入 WindowsWic；该 RW2 的 WIC 无法证明 sRGB 输出，按既有
颜色合同回退 LibRaw。这解释强制显影的秒级等待，普通 full 的大 JPEG 路径不受影响。

最终 RW2 首次 full 前触发强制显影，DOM 观测实际可见序列为
341×512 thumbnail → 4000×6000 Interim → 4008×6008 Satisfied。
从测试触发起分别在 97 / 339 / 6740 ms 观察到这些图片，包含约 90 ms 的点击调度。
最终原生结果约 6358 ms 到达，而浏览器仍保留大 JPEG 至最终解码完成；其间没有小图回退。
这些是单次过渡验证，不是显影延迟预算。

## 回归检查

覆盖 CR3 UUID/CMT 定位与截断、Canon 负坐标/多点/旋转、Panasonic tag ID/有理数/
无效标记、最大 JPEG 小于 4096 时的 preview satisfaction，以及 Interim 的浏览器
解码保留。两张真实 RAW 另验证强制显影首次返回最大 JPEG / Interim，取消后拒绝升级。

`pnpm check`、217 项前端测试、生产构建、`cargo fmt --all --check`、workspace Clippy
及 workspace tests 通过。真实夹具启用后验证，另单独运行新增的显影取消回归。
Release WebView 六次普通 full 场景完成，交互测试实例已关闭。

人脸补充后，新增大小端/数量/截断解析和缩略图坐标/旋转/多脸/AF 优先回归通过；
PANA4999 单独启用真实元数据测试，workspace 使用原有媒体夹具完成全量测试，
fmt、Clippy 与 Release 构建通过。通用 FocusInfo 和前端没有新增字段或逻辑。
