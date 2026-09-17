# Changelog

All notable changes to OxyViewer will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Each release lists the English notes first, followed by the Simplified Chinese notes.

本文件记录 OxyViewer 的所有重要变更。格式遵循 [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)，
版本号遵循[语义化版本](https://semver.org/lang/zh-CN/)。每个版本先列出英文说明，随后是同一版本的简体中文说明。

## [Unreleased]

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

[Unreleased]: https://github.com/oxygenkun/oxyviewer/compare/v0.1.3...HEAD
[0.1.3]: https://github.com/oxygenkun/oxyviewer/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/oxygenkun/oxyviewer/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/oxygenkun/oxyviewer/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/oxygenkun/oxyviewer/releases/tag/v0.1.0
