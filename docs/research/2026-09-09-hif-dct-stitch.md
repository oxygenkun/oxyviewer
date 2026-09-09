# JPEG tiles 的 DCT 系数域拼接：验证结果与接入建议

日期：2026-09-09。归档于 2026-09-10；本文保留实验当时的结论，当前实施状态见 [HIF 加速计划](../tasks/hif-performance-plan.md)。

**结论：可行，当前六块 FFmpeg JPEG tiles 已有直接实证；建议作为后台 full artifact 生成路径。** 锐化状态沿用已有 `ArtifactPresentation.sharpening`，不构成额外障碍。本轮只创建独立研究原型和实验材料，没有改动 OxyViewer 生产代码或依赖。

## 方法

libjpeg-turbo 的公开系数 API 明确支持将多 strip/tile JPEG 重组为一个 JPEG 码流。[官方 libjpeg 文档：Really raw data: DCT coefficients](https://github.com/libjpeg-turbo/libjpeg-turbo/blob/main/doc/libjpeg.txt)

本次原型对每块执行 `jpeg_read_coefficients()`，取得已量化的 8×8 DCT 系数，按屏幕坐标复制到输出各分量的块数组，最后用 `jpeg_write_coefficients()` / `jpeg_finish_compress()` 统一熵编码。

这条路线没有 HIF 解码、IDCT、RGB/RGBA 重建、像素重采样、正向 DCT 或再次量化。它包含熵解码、系数拷贝、熵编码和文件写入，所以不是零拷贝。输出相对于输入 JPEG tiles 无新增量化损失；并不声称相对于原始 HIF 无损。

没有连续调用六次 `jpegtran -drop`，因为那会重复读写大图。参考 `jpegtran` 的能力，但原型一次组合所有 tiles，只写一次输出。[官方变换实现](https://github.com/libjpeg-turbo/libjpeg-turbo/blob/main/src/transupp.c)

## 实验设置

- 用户提供的 NAS 目录中 DSC01443.HIF、DSC01444.HIF、DSC01445.HIF，使用上一轮 FFmpeg 9.0.1 生成的本地 JPEG tiles。
- 每张 7008×4672，六 tiles；分别测试未锐化和已锐化版本。
- libjpeg-turbo 3.1.3 官方源码，MSVC Release x64，NASM 2.16.03，启用 SIMD；所有工具只放在本研究目录，没有系统安装。
- 两种模式各四轮，轮次内交换执行次序；共 48 个性能样本。
- 单个独立进程执行一张拼接，峰值工作集由 GetProcessMemoryInfo 测得。每次保留最终输出；使用普通文件写入，没有做 FlushFileBuffers/fsync，也没有调用 OxyViewer 原子发布协议。
- 计时不含 HIF→tiles 阶段，不包含实际 cache fsync/commit、IPC 或 WebView 绘制。

## 耗时和大小

| 模式 | 拼接内部耗时中位数 | 含程序启动的墙钟中位数 | 文件大小 |
|---|---:|---:|---|
| 固定标准 Huffman 表（fast） | 450.8 ms | 506.7 ms | 比优化表模式大 14.8%（中位数） |
| 优化 Huffman 表（stitch） | 879.6 ms | 929.1 ms | 更紧凑 |

普通/锐化分开看：

| 输入 | fast 内部/墙钟 | 优化表内部/墙钟 |
|---|---:|---:|
| 未锐化 | 438.6 / 502.4 ms | 853.4 / 904.1 ms |
| 已锐化 | 456.7 / 511.8 ms | 902.6 / 951.6 ms |

快速模式各阶段中位数约为：熵读取 231 ms、系数拷贝 67 ms、熵写出 133 ms；其余为验证、分配和收尾。优化表模式熵写出阶段约 561 ms，是主要额外成本。这是压缩表选择，两个模式保持同样的 DCT 系数和画质。

普通样本 DSC01443：fast 5,541,769 字节，优化表 4,781,517 字节；锐化同图：fast 6,147,592 字节，优化表 5,308,580 字节。

上一轮从源 HIF 重新生成未锐化整图 JPEG约 2396 ms/张（包括 FFmpeg 进程）。本轮未锐化系数拼接墙钟约 502 ms，额外持久化阶段显著缩短，且不再读源 HIF。这是不同实现路径的本机实验比较，不是严格隔离单一变量的微基准，也不是前台首绘提速百分比。

## 正确性

1. 三张真实 HIF × 锐化开/关 × 两种编码模式，共 12 个最终文件，都重新读回输出 DCT 系数并逐块比较。每张 98,224,128 个量化系数，全部一致；输出的量化表、采样因子与输入对应分量一致。
2. 使用 Pillow 将合成 JPEG 解码成 RGB，再逐区域与分别解码的 tiles 比较：所有输出最大通道差为 0，不同通道数为 0。验证的是当前解码器下的像素一致性，不宣称所有 JPEG 解码器之间逐像素相同。
3. 从 DSC01443 的源 HEVC tiles 在 FFmpeg 中裁边、顺时针旋转并锐化，构造 4672×7008 布局；拼接后系数和 RGB 也完全一致。这是受控旋转案例，不等同于已经验证真实竖拍 HIF 的元数据解析。
4. 缺一块、块重叠、起点不对齐、画布覆盖不足、量化表不一致和损坏 JPEG 六种负例都被拒绝，且这些预检负例未生成输出文件。

## 对当前 JPEG tiles 的限制

- 所有分量的实际量化表必须兼容。不能只检查 FFmpeg 的 `q:v` 或抽象 quality 数值；不同表时不要静默重新量化，退回现有生成路线。
- 精度、分量身份、色彩解释和采样配置必须兼容。本原型仅支持 8-bit、三分量 YCbCr。
- 位置必须按 iMCU 对齐，内部边界不能包含残缺 MCU。图片最右/最下外边界的 padding 必须正确处理。
- 当前 FFmpeg 标为 yuvj444p，但 SOF 中各分量采样因子均为 1×2，实际 iMCU 为 8×16。不能仅凭“4:4:4”假定所有文件的 iMCU 都是 8×8。
- 六块使用不同 Huffman 表不是障碍：输入各自熵解码，输出统一编码即可。
- 输出坐标应使用已完成方向变换的 session tile 坐标，不能再次旋转像素或附加重复 EXIF 旋转。
- 色彩标记/ICC 必须按既有 artifact presentation 契约处理。本原型未实现通用 ICC/EXIF 搬运，当前 FFmpeg 样本通过像素验证；不能把 unknown/unconverted 数据标成已转换 sRGB。

## 内存成本

峰值工作集中位约 221.6 MiB；受控旋转样本约 224 MiB。完整输出量化系数本身约 187.35 MiB（98,224,128 × 2 字节），此外还有当前输入 tile 系数、库状态和编码缓冲。

因此“没有 RGBA”不等于“低内存”：这个三分量系数画布甚至比同尺寸 RGBA 字节数组更大。逐块解码并释放输入，避免同时保留六块输入系数；但公开 libjpeg 系数写入接口仍需要完整输出系数数组。要进一步降低峰值，需要更复杂的流式/自定义系数存储方案，不建议首版深入依赖库内部接口。

## 建议接入方式

1. 前台仍发布 JPEG tiles，保留 `(x,y,width,height,Arc<[u8]>)` 的不可变快照；后台只克隆 Arc 引用，不复制压缩字节，不从可变 session map 延迟取数据。
2. 完成事件之后经过现有 dwell、取消检测与独立后台许可，再调用系数拼接。拼接不能占用前台 decode permit。
3. 首版使用固定 Huffman 表快速模式，接受约 15% 的体积增加。仅一个 full 拼接任务同时运行；按尺寸、采样和系数数量计算内存预算，超预算跳过拼接或使用现有 fallback，不能无界申请。
4. 在 oxy-media 增加小型 C adapter，通过 libjpeg 公共 API；C 层捕获 libjpeg 错误并返回 Rust，禁止 longjmp 穿越 Rust 栈。当前原型的错误直接退出进程，只适用于独立研究，不能直接作为进程内适配层上线。
5. 从 Arc 字节使用 memory source，写入 ArtifactCache 提供的临时目标；完成后再次检查 sourceRevision/generation/cancellation，再使用既有 `publish_staged` 和原子持久化流程。坏 JPEG、中途取消或失败的部分文件必须由临时文件对象清理。
6. presentation 使用已有 `sharpening = Display/None`、orientation 和 color；full 查询按显示状态选择对应 variant，命中已锐化缓存直接显示。无需新增是否锐化字段。
7. 缺 tile、重复 tile、尺寸/量化/采样不兼容、源修改时拒绝本路径，保留当前 artifact 路线作为受控 fallback。不能用空白块填补缺失数据后宣称 full 完整。
8. 加入取消边界：每个 tile 熵读取前后、拷贝行批次及输出进度检查。不要只在任务结束后发现切图；后台仍需受内存和 CPU 并发上限约束。
9. 实现后验证缓存状态切换、源替换、异常清理、实际打包应用的后台拼接期间切图，以及 WebView 展示拼接 artifact。性能指标应区分前台完成和后台持久化。

## 取舍

优先推进该路线比先做常驻 FFmpeg worker 更直接：它复用已经生成的 JPEG tiles，减少第二次源解码，前台传输契约保持不变。常驻 worker 仍可作为独立的前台解码优化，两者并不互斥。

尚未完成：生产适配、Rust FFI 错误与取消边界、任意相机/采样配置矩阵、真实竖拍源元数据、fsync/缓存提交耗时和 WebView 实测。因此本轮结论是“算法与当前样本已验证，值得接入”，不是“生产已完成”。

## 复现材料

- `stitch.c`、`CMakeLists.txt`：独立公共 API 原型。
- `bench.py`、`results.json`：48 个性能样本。
- `verification.json`：12 个输出的系数及逐像素验证。
- `extra.py`、`extra-results.json`：受控旋转和六种负例。
- `plain-0-fast.jpg`、`sharp-0-fast.jpg`、`portrait.jpg`：样本输出。
- `libjpeg-turbo-3.1.3.tar.gz`：官方版本源码，保留上游许可证。

本轮所有实验子进程已结束，没有启动 OxyViewer debug 实例。


归档数据见 [实验数据索引](hif-benchmark-data/README.md)。原型、图片和编译工具仍保留在本次本地实验目录，不纳入仓库。
