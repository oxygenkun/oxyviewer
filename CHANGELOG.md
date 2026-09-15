# Changelog

All notable changes to OxyViewer are documented in this file. Each release lists the Simplified Chinese notes first, followed by the English notes.

本文件记录 OxyViewer 的所有重要变更。每个版本先列出简体中文说明，随后是英文说明。

## [0.1.2] - 2026-09-16

### 新增

- 设置中新增“关于”面板，显示当前版本、许可证与作者，并可按需手动检查是否有新版本。
- 每个快捷键操作可绑定两个键位，点击键位即可改键，按 Backspace 或 Delete 清空，按 Esc 取消。
- 搜索框支持按层级标签筛选，输入 `#` 可获得标签建议，并可切换“全部满足”与“任一满足”两种匹配方式。

### 修复

- 修复拖动面板、调整手柄或放大镜时界面元素被当作文本选中的问题，输入框与错误详情仍可正常选中复制。

### Added

- Added an About panel in Settings that shows the running version, license, and author, and can check for a new release on demand.
- Each shortcut action can bind two keys: click a key to rebind, press Backspace or Delete to clear it, and press Escape to cancel.
- Search now filters by hierarchical tags, with tag suggestions when typing `#` and a switch between matching all and matching any tag.

### Fixed

- Dragging panels, resize handles, or the loupe no longer selects interface elements as text, while inputs and error details remain selectable.

## [0.1.1] - 2026-09-14

### 新增

- 新增可自定义快捷键，并可在设置中查看全部可用快捷键。
- 新增旗标筛选功能。

### 修复

- 改进佳能 CR3 文件的元数据读取，包括色温、色调（白平衡偏移）和自动亮度优化（DRO）。
- 修复切换放大镜视图时缩放比例异常跳动的问题。
- 修复状态栏中所选文件坐标数值显示异常的问题。
- 修复在横向与纵向布局之间切换时，所选缩略图无法保持可见位置的问题。

### Added

- Added customizable keyboard shortcuts, with all available shortcuts now listed in Settings.
- Added flag-based filtering.

### Fixed

- Improved metadata parsing for Canon CR3 files, including color temperature, tint (white balance shift), and Auto Lighting Optimizer (DRO).
- Fixed unexpected zoom level changes when switching between Loupe views.
- Fixed coordinate values for the selected file in the status bar.
- The selected thumbnail now stays in view when switching between horizontal and vertical layouts.

## [0.1.0] - 2026-09-13

### 新增

- 本地优先、分页的文件夹浏览，支持虚拟化缩略图网格与胶片条。
- JPEG、HEIF 和 RAW 预览工作流，支持渐进式全细节查看。
- 平台感知的图像解码、色彩管理、方向处理与对焦区域显示。
- 元数据查看，并可按评分、颜色标签和层级标签筛选。
- 持久化图库索引、文件夹搜索、收藏与自定义文件夹排序。
- 照片评分、标记、打标签、重命名、删除及外部应用操作。
- 可重建的媒体与图库缓存，支持可配置的存储管理和队列诊断。
- macOS 与 Linux 安装包，以及双语 Windows NSIS EXE 和便携 ZIP 包，内置固定版本的 FFmpeg 工具；对应的 FFmpeg 源码作为独立发布资源提供。

### Added

- Local-first, paged folder browsing with a virtualized thumbnail grid and filmstrip.
- JPEG, HEIF, and RAW preview workflows with progressive full-detail viewing.
- Platform-aware image decoding, color management, orientation handling, and focus-area display.
- Metadata inspection and filtering by rating, color label, and hierarchical tags.
- Persistent library indexing, folder search, favorites, and custom folder ordering.
- Photo rating, labeling, tagging, renaming, deletion, and external-application actions.
- Rebuildable media and library caches with configurable storage controls and queue diagnostics.
- macOS and Linux installers plus bilingual Windows NSIS EXE and portable ZIP packages with bundled, pinned FFmpeg tools; corresponding FFmpeg source is published as a separate release asset.
