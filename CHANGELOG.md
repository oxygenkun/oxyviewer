# Changelog

All notable changes to OxyViewer will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Each release lists the English notes first, followed by the Simplified Chinese notes.

本文件记录 OxyViewer 的所有重要变更。格式遵循 [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)，
版本号遵循[语义化版本](https://semver.org/lang/zh-CN/)。每个版本先列出英文说明，随后是同一版本的简体中文说明。

## [Unreleased]

## [0.2.2] - 2026-11-26

### Added

- Added a guided tour for importing folders and using Grid, Loupe, and the Inspector. The tour can be restarted from General settings.
- Added adjustable trackpad pinch sensitivity and two-finger panning for zoomed photos in Loupe.
- Added configurable full-image predecoding in Loupe. After the current image is ready, Loupe predecodes up to 20 photos in the browsing direction, with a default of two.

### Fixed

- Moved original-file dragging in Loupe to the filmstrip, so dragging the main photo can pan a zoomed image.
- Removed automatic RAW decoder prompts from Loupe. Decoder setup and retry are available in Settings.

### 新增

- 新增入门导览，介绍如何导入文件夹以及使用网格、Loupe 和详细信息栏。可在常规设置中重新开始导览。
- Loupe 支持调节触摸板捏合缩放灵敏度，放大照片后可用双指滑动平移。
- Loupe 支持设置完整图像的预解码张数。当前照片就绪后，沿浏览方向预解码后续照片，默认 2 张，最多 20 张。

### 修复

- 将 Loupe 的原始文件拖出操作移至胶片条，拖动放大后的主图可平移照片。
- Loupe 浏览 RAW 时取消自动显示解码器提示，仍可在设置中配置解码器和重试显影。

## [0.2.1] - 2026-11-25

### Added

- Added configurable shortcuts for mouse buttons and wheel directions in Grid, Loupe, and marking controls.
- Added an option to show person tags in the tag picker and its search results. These suggestions are hidden by default.
- Introduced a refreshed desktop interface with translucent panels, centered search, and mixed-format browsing in Grid and Loupe.
- Added the active photo path and RAW/HEIF preview status to the status bar. The inspector starts with Basic info and labels its ISO and exposure compensation tile Exposure.
- Added hover previews to the stars used for rating and filtering photos.

### Fixed

- Restored dragging original files from Windows network folders.
- Improved keyboard focus and navigation in shared menus and Settings.
- Fixed macOS packaging to ad-hoc sign the application and verify the copy inside the DMG before publication.

### 新增

- 网格、Loupe 和标记操作支持将鼠标按键与滚轮方向设为快捷键。
- 标签选择器可在候选列表和搜索结果中显示人物标签，默认隐藏这些候选项。
- 更新桌面界面，采用半透明面板和居中搜索栏，网格与 Loupe 混合显示各格式照片。
- 底栏显示当前照片路径和 RAW／HEIF 预览状态。检查器从“基础信息”开始，并将显示 ISO 和曝光补偿的格子命名为“曝光”。
- 评分和评分筛选的星星支持悬停预览。

### 修复

- 修复 Windows 下无法从网络文件夹拖出原始文件的问题。
- 改进共享菜单和设置中的键盘焦点与导航。
- 修复 macOS 打包流程，发布前对应用进行临时签名并验证 DMG 内的应用副本。

## [0.2.0] - 2026-11-04

### Added

- Added repeating the last tag on the current selection through a configurable shortcut.
- Added native file dragging from the grid and the loupe, with grid multi-selection and Alt-drag from a zoomed photo.
- Added folder-wide sidecar tag writeback with progress, cancellation, and per-file results.
- Added the XMP mirror queue state to the inspector header, with a retry action when a write fails.
- Added an undo toast for photo and tag deletion, replacing the confirmation panel with a three-second window that can be cancelled.
- Added recent, most-used, and tree ordering to the tag picker, a remembered choice, and hierarchical tag creation from the search row.
- Added a recheck action to the ExifTool setup prompt, so a manually installed binary is detected without restarting the app.
- Sidecar ratings and tags are now read while browsing, so files with adjacent XMP keep their markings without a full rebuild.

### Changed

- Moved application state to the isolated `core-v2` data directory with schema 2. Library state written by 0.1.x is rejected read-only instead of migrated, so folders must be added again to rebuild the library.

### Fixed

- Fixed tag removal so manual and sidecar assignments are withdrawn together and the updated keywords are written back to XMP, while person contributions keep the tag.
- Fixed XMP keyword arrays whose members contain commas, non-ASCII text, or duplicates being split into separate tags.
- Fixed verified person identities detaching from their photos after a file rename.
- Fixed person tag contributions being lost during library folder recovery.

### 新增

- 新增重复上一个标签的快捷键，可自定义并与当前选择联动。
- 新增从网格与放大镜拖出原始文件，网格支持多选拖出，放大镜支持按住 Alt 拖出。
- 新增整文件夹的 sidecar 标签回写，提供进度、取消与逐文件结果。
- 新增信息面板标题栏的 XMP 同步队列状态，写入失败时可重试。
- 新增照片与标签删除的撤销提示，以三秒可取消的窗口取代确认面板。
- 新增标签选择器的最近使用、最常使用与树形排序，并记住选择；支持从搜索行创建层级标签。
- 新增 ExifTool 安装提示中的重新检查操作，手动安装后无需重启即可识别。
- 浏览时直接读取 sidecar 中的评分与标签，存在相邻 XMP 的文件无需重建即可显示标记。

### 调整

- 应用状态迁移到隔离的 `core-v2` 数据目录与 schema 2。0.1.x 写入的图库状态会被只读拒绝而不迁移，需要重新添加文件夹以重建图库。

### 修复

- 修复删除标签时仅移除部分来源的问题：现在同时撤回手动与 sidecar 赋值并回写 XMP，同时保留人物贡献的标签。
- 修复 XMP 关键字数组成员包含逗号、非 ASCII 文本或重复项时被拆分为多个标签的问题。
- 修复已验证的人物身份在文件重命名后与照片脱钩的问题。
- 修复图库文件夹恢复过程中丢失人物标签贡献的问题。

## [0.1.5] - 2026-10-10

### Added

- Added an experimental People module, disabled by default and available from Settings → General → Feature modules.
- Added app-wide person identities, reference galleries, face/body annotations, batch candidate review, and shared Grid/Loupe person filtering.
- Added explicit folder face analysis, anonymous face grouping, and selected-person candidate retrieval on Windows x64, with model installation, progress, and cancellation controls. Automatic results remain candidates for manual confirmation; macOS inference is not yet supported.
- Added recovery for unavailable library folders, including retry and relinking to a new location while preserving annotations.

### Changed

- Unified shared buttons, fields, dialogs, and folder menus for more consistent interaction.
- Separated persistent user records from rebuildable caches through dedicated storage, tag, and person domains.

### Fixed

- Corrected focus-area placement when the displayed preview dimensions differ from the source image.
- Preferred XMP values when displaying capture metadata.
- Corrected macOS ImageIO decoding of transparent images to preserve stable alpha and avoid darkened translucent colors.
- Preserved unavailable-folder state across restarts and rechecked folders on refresh, with separate retry and relocation actions.

### 新增

- 新增实验性人物模块，默认关闭，可在“设置 → 常规 → 功能模块”中开启。
- 新增全 App 人物身份、参考图库、人脸／人体标注、批量候选审阅，以及网格与放大镜共用的人物筛选。
- Windows x64 支持显式启动文件夹人脸分析、匿名人脸分组和指定人物候选检索，提供模型安装、进度与取消操作。自动结果仍需人工确认；macOS 推理尚未支持。
- 新增失效图库文件夹恢复，支持重试和重新关联位置，并保留标注。

### 调整

- 统一按钮、表单、对话框和文件夹菜单，改善交互一致性。
- 将持久化用户记录与可重建缓存分离，并拆分存储、标签和人物领域。

### 修复

- 修复显示预览尺寸与原图不同时，对焦区域位置不准确的问题。
- 显示拍摄元数据时优先采用 XMP 中的值。
- 修复 macOS ImageIO 读取透明图片时 alpha 不稳定及半透明颜色变暗的问题。
- 重启后保留失效文件夹状态，刷新时重新检查，并区分重试与重新关联操作。

## [0.1.4] - 2026-09-21

### Added

- Removed the list view mode.
- The grid supports grouping Sony burst shots for viewing.
- The grid and filmstrip support deleting multiple selected assets.
- Thumbnail orientation preferences are remembered separately for each library folder.

### Fixed

- Fixed the UI breaking when assets are deleted.

### 新增

- 移除列表查看模式
- 网格支持 Sony 连拍合并查看
- 网格与胶片条支持多选删除
- 缩略图方向偏好会按图库文件夹分别记忆。

### 修复

- 修复资源删除时界面崩坏问题


## [0.1.3] - 2026-09-17

### Added

- Folders can be imported by dropping them onto the window, with progress, per-folder results, and retry in the status bar.

### 新增

- 支持拖入文件夹导入；状态栏显示导入进度与各文件夹结果，失败可重试。

## [0.1.2] - 2026-09-16

### Added

- Added an About panel with version information and update checking.
- Improved keyboard shortcut settings.
- Added tag filtering.

### Fixed

- Fixed an issue where UI control content could be accidentally selected.

### 新增

- 新增“关于”面板，可查看版本信息并检查更新。
- 优化快捷键设置功能。
- 新增标签筛选功能。

### 修复

- 修复应用界面控件内容可能被意外选中的问题。

## [0.1.1] - 2026-09-14

### Added

- Added customizable keyboard shortcuts, with all available shortcuts now listed in Settings.
- Added flag-based filtering.

### Fixed

- Improved metadata parsing for Canon CR3 files, including color temperature, tint (white balance shift), and Auto Lighting Optimizer (DRO).
- Fixed unexpected zoom level changes when switching between Loupe views.
- Fixed coordinate values for the selected file in the status bar.
- The selected thumbnail now stays in view when switching between horizontal and vertical layouts.

### 新增

- 新增可自定义快捷键，并可在设置中查看全部可用快捷键。
- 新增旗标筛选功能。

### 修复

- 改进佳能 CR3 文件的元数据读取，包括色温、色调（白平衡偏移）和自动亮度优化（DRO）。
- 修复切换放大镜视图时缩放比例异常跳动的问题。
- 修复状态栏中所选文件坐标数值显示异常的问题。
- 修复在横向与纵向布局之间切换时，所选缩略图无法保持可见位置的问题。

## [0.1.0] - 2026-09-13

### Added

- Local-first, paged folder browsing with a virtualized thumbnail grid and filmstrip.
- JPEG, HEIF, and RAW preview workflows with progressive full-detail viewing.
- Platform-aware image decoding, color management, orientation handling, and focus-area display.
- Metadata inspection and filtering by rating, color label, and hierarchical tags.
- Persistent library indexing, folder search, favorites, and custom folder ordering.
- Photo rating, labeling, tagging, renaming, deletion, and external-application actions.
- Rebuildable media and library caches with configurable storage controls and queue diagnostics.
- macOS and Linux installers plus bilingual Windows NSIS EXE and portable ZIP packages with bundled, pinned FFmpeg tools; corresponding FFmpeg source is published as a separate release asset.

### 新增

- 本地优先、分页的文件夹浏览，支持虚拟化缩略图网格与胶片条。
- JPEG、HEIF 和 RAW 预览工作流，支持渐进式全细节查看。
- 平台感知的图像解码、色彩管理、方向处理与对焦区域显示。
- 元数据查看，并可按评分、颜色标签和层级标签筛选。
- 持久化图库索引、文件夹搜索、收藏与自定义文件夹排序。
- 照片评分、标记、打标签、重命名、删除及外部应用操作。
- 可重建的媒体与图库缓存，支持可配置的存储管理和队列诊断。
- macOS 与 Linux 安装包，以及双语 Windows NSIS EXE 和便携 ZIP 包，内置固定版本的 FFmpeg 工具；对应的 FFmpeg 源码作为独立发布资源提供。

[Unreleased]: https://github.com/oxygenkun/oxyviewer/compare/v0.2.2...HEAD
[0.2.2]: https://github.com/oxygenkun/oxyviewer/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/oxygenkun/oxyviewer/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/oxygenkun/oxyviewer/compare/v0.1.5...v0.2.0
[0.1.5]: https://github.com/oxygenkun/oxyviewer/compare/v0.1.4...v0.1.5
[0.1.4]: https://github.com/oxygenkun/oxyviewer/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/oxygenkun/oxyviewer/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/oxygenkun/oxyviewer/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/oxygenkun/oxyviewer/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/oxygenkun/oxyviewer/releases/tag/v0.1.0
