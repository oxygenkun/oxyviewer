# Changelog

All notable changes to OxyViewer are documented in this file. Each release lists the Simplified Chinese notes first, followed by the English notes.

本文件记录 OxyViewer 的所有重要变更。每个版本先列出简体中文说明，随后是英文说明。

## [0.1.1] - 2026-09-14

### 新增

- 可自定义的键盘快捷键，覆盖 loupe、网格和列表视图，支持按键录制改键、单项与全局重置、内置别名、按作用域检测冲突，并持久保存绑定。
- loupe 中空格键循环切换缩放，已解析对焦区域时以其为锚点；网格或列表中按空格可用 loupe 打开当前照片。
- 工具栏与资源查询支持 pick 标记筛选，组内按“任一选中标记”匹配。
- 从 RAW MakerNotes 提取佳能色温、色调（白平衡偏移）和自动亮度优化（DRO）。

### 修复

- loupe 的像素缩放百分比改为基于文件真实尺寸，切换到 RAW 文件时不再在全像素加载前闪现瞬时比例。
- 状态栏显示所选照片在筛选后文件夹顺序中的位置，而不是已加载缩略图的数量。
- 缩略图在横竖方向之间切换时，网格、列表和胶片条保持所选照片可见。

### Added

- Customizable keyboard shortcuts for the loupe, grid, and list views, with press-to-rebind capture, per-action and global reset, built-in aliases, scope-aware conflict detection, and persisted bindings.
- Space-bar zoom cycling in the loupe, anchored on the shooting focus region when one is parsed, and space opens the active photo in the loupe from the grid or list.
- Pick-flag filtering in the toolbar and asset query, matching any selected flag within the group.
- Canon color temperature, tint (WB shift), and Auto Lighting Optimizer extraction from RAW MakerNotes.

### Fixed

- The loupe's pixel-zoom percentage now derives from the file's real dimensions, so switching to a RAW file no longer flashes a transient scale before the full pixels load.
- The status bar shows the selected photo's position within the filtered folder order instead of the number of thumbnails loaded so far.
- The grid, list, and filmstrip keep the selected photo in view when thumbnail orientation changes between landscape and portrait.

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
