# 前端基础控件与样式

OxyViewer 保留原生 CSS，通过 `styles/foundation.css` 统一语义颜色、控件尺寸、
间距、圆角和顶层浮层层级。普通控件采用紧凑规范；缩略图、画布与拖动区域的几何
仍由业务组件管理。不要把不同用途的颜色或局部 stacking context 强行合并。

## 公共组件

`apps/desktop/src/components/ui` 提供：

- `Button`：secondary、primary、danger、ghost，small/regular 尺寸；默认
  `type="button"`。loading 保留文字、显示图标、标记 aria-busy 并禁止重复操作。
- `IconButton`：要求 label，为纯图标操作提供可访问名称。
- `Field`：单个 input 的 label、hint、error、suffix，自动关联 id 与描述。
  多个选项仍应使用 fieldset；不要用它包装复合选择器。
- `Dialog`：基于 `@radix-ui/react-dialog` 的受控模态窗口，提供标题、描述、
  焦点限制、初始焦点和关闭后焦点恢复。调用方必须明确是否允许点击背景关闭。
  键盘事件在弹窗内停止冒泡，避免触发背景快捷键；Escape 由 Radix 处理。
  没有持久 Trigger 时会记录打开前焦点，也可显式提供 returnFocusRef。
  返回目标已被移除时不会聚焦失效 DOM。

## 渐进迁移

基础组件及删除确认使用同目录 `*.module.css`，由 Vite 处理。
全局 CSS 只保留基础规则、现有区域布局和共享变量；新增组件优先使用 CSS Modules。
公共组件拥有自己的状态样式，不在业务 CSS 中通过后代 button/input 选择器覆盖。

第一批接入：删除确认、设置关闭按钮、缓存操作与容量字段、外部应用操作与编辑字段、
RAW 操作按钮。删除确认默认聚焦取消；点击背景、Escape、取消按钮均可关闭；
仅确认按钮触发文件操作。其专属旧全局样式已移除。

文件夹标题栏的更多操作使用 Radix Dropdown Menu，包含排序 RadioGroup 子菜单和
拖动开关 CheckboxItem。定位、边缘避让、方向键导航和关闭后的焦点返回由 Radix 管理；
菜单使用独立 CSS Module，键盘事件不冒泡到照片快捷键。

设置窗口、人物管理与其他菜单尚未整体迁移到 Radix；后续按组件逐步接入，
不同时引入 Tailwind 或另一套成套视觉系统。

## 验证

运行 CONTRIBUTING.md 中的 `pnpm check`、`pnpm test`、`pnpm build`。
基础测试覆盖 loading 防重入、字段关联、Tab 循环、快捷键隔离及 Escape 焦点恢复；
AssetBrowser 测试覆盖确认删除的原有选择范围。
UI 变更还应在浏览器检查背景关闭、Shift+Tab、长文件名和设置页的真实布局。
