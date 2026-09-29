# Windows ML 后端速度比较（2026-09-29）

## 结论

本机 RTX 4090 的 TensorRT RTX 热运行最快：45–52 ms/图，相对同轮普通 CPU
（应用当前的 2 线程配置）快 4.3–4.9 倍。DirectML / RTX 4090 为 57–70 ms/图。
但 TensorRT RTX 建立两个模型 session 需约 8.6 秒，DirectML / RTX 4090 为
1.6–3.0 秒；小批次必须计入这个启动成本，不能只按热运行吞吐选择。

Intel UHD 770 的 OpenVINO GPU 为 299–336 ms/图，而且 AdaFace 输出与 CPU
显著不一致，不能作为可用加速结果。独立进程重复得到相同的最小余弦 -0.05187247；
普通 OpenVINO GPU 和 `.AUTO` GPU 路径均复现。错误根因尚未定位，不能据此断言
所有 Intel 设备或所有 OpenVINO 模型都有问题。OpenVINO CPU 输出正常，与普通
CPU 的速度接近。

本次只增加 opt-in 基准和记录，没有按测试结果改动产品 EP 优先级。
现有启用前试运行检查的是成功运行及有效形状／归一化，并不能发现这种跨后端
特征偏差；因此 Intel GPU 编码路径仍需修复或增加精度资格检查。

## 测试条件

- Windows、Intel Core i7-13700K（16 核／24 逻辑处理器）、RTX 4090、UHD 770。
- NVIDIA 驱动 `32.0.16.1714`，Intel 驱动 `32.0.101.7088`。
- Release Rust，应用固定 Windows ML 2.4.89、ORT API 27，应用实际 SCRFD 与
  AdaFace WebFace12M ONNX，摘要均校验。每个 ORT session 的 intra_threads 为 2；
  厂商 EP 自有线程／内部调度仍由其默认实现管理，未调优或限制为两个硬件线程。
- 从 dataset01 全部 JPEG 的排序列表均匀抽取 16 张，覆盖零脸、单脸和双脸；
  每轮共检测 16 张脸。文件名、SHA-256、尺寸和结果保存在
  [原始数据](2026-09-29-winml-backend-speed.json)。
- JPEG 只预解码一次，Triangle 缩放至长边 1600，相同 RGB 常驻内存供所有后端使用。
  每个后端先处理首张图片预热，然后顺序执行 3 轮：48 次检测、48 次编码。
- 检测画布 960、score 0.5、NMS 0.4；编码统一使用 CPU 检测的五点，保证人脸
  裁剪和编码工作量完全一致。另行检查各后端检测数量和同输入特征余弦。
- 总耗时是每图检测与所有人脸编码耗时之和，包括预处理、ORT Run、CPU/GPU
  传输及检测后处理／特征归一化；它不是纯 GPU kernel 时间。
- 不含 JPEG 读取／解码、1600 缩放、SQLite、任务账本和 UI。统一输入准备约
  11.1–11.2 秒／16 张，使用 `image` 全图解码，与应用 `oxy-media` 的 scaled IDCT
  路径不同，不能加到表内来冒充应用端到端耗时。没有重测 NAS、混合格式或浏览卡顿。
- 第一进程完成所有目录设备；目录暴露了多个相同 NVIDIA DirectML 设备条目，
  分别测得 57–70 ms/图。第二进程去重并把 RTX 提前，复测 CPU、RTX 和 OpenVINO。
  当前基准已过滤重复条目与 `.AUTO` 别名。
- 未固定 CPU 核心亲和性／频率、GPU 时钟，也未独占整机；两进程 CPU 结果有明显
  波动，因此报告范围并按同一进程的 CPU 基线计算加速比，不宣称实验室级确定值。

## 热运行结果（平均耗时）

“检测”包含 SCRFD 预处理和后处理；“编码”是每张人脸的对齐与特征提取。
各批次平均每图一张脸，所以总平均恰好等于两列之和。

| 后端 | 首轮检测 ms/图 | 首轮编码 ms/脸 | 首轮合计 ms/图 | 复测合计 ms/图 | 输出检查 |
| --- | ---: | ---: | ---: | ---: | --- |
| CPU，2 线程 | 201.75 | 56.47 | 258.23 | 190.75 | 基准 |
| TensorRT RTX / RTX 4090 | 49.30 | 3.06 | 52.36 | 44.79 | 通过 |
| DirectML / RTX 4090，首个设备条目 | 66.65 | 3.31 | 69.96 | — | 通过；其他同卡条目合计 57.42／60.14／67.75 |
| OpenVINO / Intel CPU | 191.83 | 48.09 | 239.92 | 193.60 | 通过 |
| OpenVINO / UHD 770 | 228.48 | 107.27 | 335.75 | 298.53 | **特征不一致，不可用** |
| DirectML / UHD 770 | 372.52 | 105.37 | 477.89 | — | 通过，但比 CPU 慢 |

RTX 完成热运行 16 张一批约 0.72–0.84 秒，CPU 约 3.05–4.13 秒。
复测 RTX 每图 p50 44.23 ms、p95 49.91 ms；CPU p50 187.89 ms、p95 231.60 ms。
所有后端的检测数量都一致，这不能证明框坐标完全一致或人物识别质量已校准。
RTX 最小特征余弦 0.99999839，DirectML / NVIDIA 0.99999940，OpenVINO CPU
0.99999952，DirectML / Intel 0.99999827；OpenVINO GPU 为 -0.05187247。

## 启动成本

下表单独计量两模型校验及 session 建立，不包含 EP 目录扫描／注册（本次已安装
组件，每进程另花约 2.3–2.7 秒）及预热试运行。新设备首次下载也不在其中。

| 后端 | 两模型准备 | 首图检测＋编码 |
| --- | ---: | ---: |
| CPU | 1.62–1.80 s | 194–292 ms |
| TensorRT RTX / RTX 4090 | 8.56–8.63 s | 65–80 ms |
| DirectML / RTX 4090 | 1.57–2.96 s | 75–114 ms |
| OpenVINO CPU | 1.42–1.70 s | 详见原始数据 |
| OpenVINO / UHD 770 | 初测 16.29 s，复测 2.84 s | 312–316 ms，输出不合格 |
| DirectML / UHD 770 | 41.79 s | 详见原始数据 |

按两轮数据估算，仅与普通 CPU 比，RTX 额外的 session 建立成本大约需 34–47 张
照片的热运行节省来摊平。与 DirectML 比则需要更大的批次；具体收益还会受人脸数量、
媒体准备、会话复用及后台负载影响。这是算术估算，不是主应用整夹实测。

## 复现与验证

```powershell
$env:OXY_TEST_MODEL_ROOT = "$env:APPDATA/app.oxyviewer.desktop/person-models"
$env:OXY_BENCH_IMAGE_DIR = "$PWD/model-research/datasets/dataset01/images"
$env:OXY_BENCH_OUTPUT = "$PWD/.test-tmp/winml-backend-speed.json"
cargo test -p oxy-people --release compare_installed_backends -- --ignored --nocapture
```

可用 `OXY_BENCH_EPS=NvTensorRTRTXExecutionProvider,OpenVINOExecutionProvider`
筛选厂商 EP；普通 CPU 基准始终运行。每个后端结束即保存 JSON，失败也记录原因。
`output_check_passed` 必须单独检查：benchmark 测试完成不代表所有后端精度合格。

`cargo fmt --all --check` 与人物模块 `clippy --all-targets --no-deps -- -D warnings`
均通过。本次没有启动桌面调试应用，也没有修改模型或原照片。
