# Windows RAW 完整解析与系统扩展

OxyViewer 随程序构建并携带 LibRaw，不要求用户先安装相机 codec。Windows 的
Raw Image Extension 是可选加速组件，由用户通过 Microsoft Store 安装和维护。
应用不下载或旁加载第三方 codec，不在启动或打开目录时弹出安装向导。

## 解析顺序

普通 `full` 请求先复用合格的完整缓存或提取最大内嵌 JPEG；相机 JPEG 在两个边上
覆盖参考尺寸的 90% 即满足现有 full 契约。需要显影时，Windows 固定依次尝试
WIC、内置 LibRaw。Thumbnail 和 Preview 的后端顺序不变。

CR3 的 JPEG track 候选有时只给偏移与长度，宽高为零。选择前按候选范围有界读取 JPEG
SOF，补齐真实尺寸；不先解码像素。RW2 同时识别 IFD0 `0x002e` / `0x0127`
（`JpgFromRaw` / `JpgFromRaw2`），准确去掉 JPEG EOI 之后的对齐填充，保留方向与颜色元数据。
最大的合格相机 JPEG 直接经本机资源协议交给 WebView；不会执行 RAW 显影、锐化或 JPEG 重编码。

微软当前 RAW codec 没有 `IWICDevelopRaw`，因此使用经过实测的 Microsoft RAW
decoder CLSID 完整帧入口。必须取得有效方向、明确的 sRGB ColorSpace 和 RGB24
像素；若 LibRaw 可读参考尺寸，帧的两个边还须与其相差不超过约 2%。其他 codec
必须提供 `IWICDevelopRaw`，成功设置 AsShot、BestQuality 和 sRGB，才允许进入
完整帧解码。任一条件不满足、格式不支持或解码失败，继续 LibRaw。

WIC 的完整帧经方向纠正和带 sRGB ICC 的 JPEG 编码后，
才能发布为 `RawSensor / native / Satisfied`。颜色声明无法确认时回退；扩展已安装
不等于每一种相机文件都能通过上述检查。两种显影的色彩、噪声和细节可能不同。

RAW 保留解码器输出，不额外锐化、降噪或调整对比度。显示锐化仅限已与官方解码结果
对比验证的 Sony HIF，不把该后处理推广到 RAW 或其他未验证格式。历史生成产物通过
设置中的“清空缓存”统一清理，不为过去的处理问题保留专门的缓存兼容分支。

## 用户流程

- 设置中的“RAW 完整解析”显示系统检测状态及可用 codec。显影顺序固定为
  WIC → LibRaw，前端不选择解码器。
- 真正尝试系统显影失败后，放大镜显示可关闭的提示；已显示的照片仍保留。
  单个文件不受支持与系统缺少 codec 分开描述，不要求为单文件错误反复安装。
- 缺少组件或检测失败时，提供 Microsoft Store 和
  [官方安装网页](https://apps.microsoft.com/detail/9nctdw2w1bh8)。商店协议打开失败
  会尝试官方网页；两个入口都失败时保留错误和内置 LibRaw 路径。
- 用户打开安装入口后，窗口重新获得焦点会重新检测，也可手动点击“重新检测”。
  WIC 枚举使用 Refresh；仍未识别时提示完成安装并重启应用，企业禁用商店时可
  继续使用内置后端。检测不自动安装，不自动重新显影。
- “重新显影当前图片”显式绕过相机 JPEG 和旧的完整显影缓存，重新执行 WIC → LibRaw 回退链生成
  新的 full。仅当前源 revision 获得重试标识，取消对应 pending/active full；
  保留缩略图、当前图片及其资源 lease。缓存仅匹配流水线给出的精确产物身份，不识别解码器名称。
- 全部显影失败时继续保留可用图片，提供重新检测、重新显影与复制诊断信息。
  剪贴板不可用时展示可选中的诊断文本。提示被关闭后仍可在设置中操作。

## 运行与缓存约束

系统枚举、无文件的 decoder 实例化探测和解码均在 blocking worker 上执行，COM
对象不跨线程或被全局保存。注册信息存在但组件无法加载时不计为可用；有当前
文件时还匹配 codec 声明的扩展名，避免只有 DNG codec 却声称支持 ARW。
状态锁仅保护小规模快照；目录打开不执行 RAW 检测。文件失败缓存最多 64 项、
30 秒抑制重复失败，刷新或当前文件重试可清除。重试标识在本进程最多保留 256
个源 revision；重启后恢复正常缓存策略；不保存后端选择偏好。

WIC 没有可靠的同步 `CopyPixels` 中断接口。保留既有 RAW full 并发 gate，取消在
native 调用前后检查，取消结果不发布，也不再启动 LibRaw。当前同步调用结束前
仍占用该 gate；未引入无法回收线程的伪超时。未知硬件和其他相机仍需扩充验证。

公共 JPEG writer 使用 64 KiB 缓冲，显式 flush 后再 sync 和原子提交；ICC、质量、
取消与不可覆盖契约不变。性能数据及复现方法见
[Windows RAW WIC 基准](research/windows-raw-wic-benchmark-2026-09-12.md)。
