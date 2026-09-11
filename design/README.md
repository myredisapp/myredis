# 麦地缓存 Logo 设计与生成说明

本目录存放麦地缓存（MaiDi Cache）Logo 的设计探索稿与生成脚本。

## 最终方案

**选定方案：Connected 徽章**

![Connected Logo](../assets/logo/logo.svg)

设计说明：
- 主体是一个带两道条纹的数据库圆柱，直接表达“数据库管理工具”属性。
- 右下角叠加一个圆形 MR 徽章，M 取自“麦 / Mai”，R 取自“Redis”。
- 圆柱与徽章之间用连接线和节点相连，强调“连接管理”这一核心功能。

## 文件结构

```
design/
├── README.md                          # 本文件
├── generate-logo-html.py              # 生成所有设计稿 HTML 的脚本
├── barley-logo-designs.html           # 第一辑：大麦形态方案
├── barley-logo-designs-v2.html        # 第二辑：符号化方案
├── barley-logo-designs-v3.html        # 第三辑：一笔成形方案
├── barley-logo-mr.html                # MR 组合方案
├── barley-logo-connection.html        # 连接管理主题方案
├── barley-logo-db-mr.html             # 数据库圆柱 + MR 方案
└── barley-logo-mr-optimized.html      # MR 徽章 / 顶标 8×8 优化方案
```

最终产物位于：

```
assets/logo/
├── logo.svg           # 主 Logo 源文件（可编辑 CSS 变量）
├── logo-themed.svg    # 主题自适应版（跟随页面 --accent）
├── favicon.svg        # 标签页图标源文件
├── icon-source.svg    # Tauri 桌面应用图标源文件（固定深秋色）
└── README.md          # Logo 使用说明
```

## 如何修改最终 Logo

### 1. 直接编辑 SVG

打开 `assets/logo/logo.svg`，修改顶部的 CSS 变量：

```css
:root {
  --md-logo-primary: #f5b342;
  --md-logo-secondary: #f9c75e;
  --md-logo-surface: #ffffff;
  --md-logo-stripe: rgba(255, 255, 255, 0.35);
}
```

所有使用这些变量的图形都会同步更新。

### 2. 用设计软件编辑

用 Figma、Sketch、Adobe Illustrator 打开 `.svg`：
- 图形为矢量路径，可任意缩放。
- 颜色已分组，可直接选中改色。
- 字体使用系统默认无衬线字体，如没有对应字体不影响显示。

### 3. 修改标题栏内联 Logo

`frontend/index.html` 标题栏中的 Logo 是内联 SVG，搜索 `class="logo-mark"` 即可找到。

如需替换为新的 SVG，直接替换该 `<svg>...</svg>` 块即可。

## 如何重新生成设计稿

如果你需要基于脚本重新生成 `barley-logo-mr-optimized.html`：

```bash
python3 design/generate-logo-html.py
```

脚本会读取 `design/generate-logo-html.py` 中定义的 `BADGE_VARIATIONS` 和 `TOP_VARIATIONS`，
输出到 `barley-logo-mr-optimized.html`。

### 添加新的变体

编辑 `generate-logo-html.py`，在 `BADGE_VARIATIONS` 或 `TOP_VARIATIONS` 列表中追加三元组：

```python
(
    "变体中文名",
    "EnglishName",
    lambda i: f'''
        <svg 内容，使用 #g{i} 作为当前主题渐变>
    '''
)
```

其中 `i` 为主题索引：
- 0: 春分（绿）
- 1: 立夏（蓝）
- 2: 初秋（金）
- 3: 深秋（琥珀，默认）
- 4: 冬至（蓝）

## 导出应用图标

### Tauri 桌面端

本项目使用 Tauri 构建桌面应用。生成所有平台图标的命令：

```bash
cargo tauri icon assets/logo/icon-source.svg
```

该命令会覆盖 `src-tauri/icons/` 下的：
- `icon.ico`（Windows）
- `icon.icns`（macOS）
- `icon.png` 及各尺寸 PNG（Linux / 通用）
- `ios/`、`android/`（如启用移动端）

`icon-source.svg` 使用固定深秋色，避免不同渲染器对 CSS 变量解析不一致。

### 网页端 / 手动导出

用浏览器或设计软件打开 `assets/logo/logo.svg` / `assets/logo/favicon.svg`，导出为所需尺寸：

| 尺寸 | 用途 |
|---|---|
| 16×16, 32×32, 48×48 | 浏览器 favicon.ico |
| 256×256 | 桌面快捷方式 |
| 512×512 | 应用商店 |

### 命令行（需 ImageMagick）

```bash
magick -background none assets/logo/favicon.svg -resize 32x32 assets/logo/favicon.ico
```

## 设计迭代记录

| 轮次 | 文件 | 方向 |
|---|---|---|
| 1 | `barley-logo-designs.html` | 大麦形态：一穗、麦浪、穗结、一粒 |
| 2 | `barley-logo-designs-v2.html` | 符号化：麦芒、丰印、双穗、粒阵 |
| 3 | `barley-logo-designs-v3.html` | 一笔成形：一笔、穗冠、圆穗、层穗 |
| MR | `barley-logo-mr.html` | 在 v3 穗冠基础上融合 MR 字母 |
| 连接 | `barley-logo-connection.html` | 强调连接管理：节点、数据库、树、环 |
| DB+MR | `barley-logo-db-mr.html` | 数据库圆柱 + MR 字母多种放置 |
| 优化 | `barley-logo-mr-optimized.html` | MR 徽章 / MR 顶标 各 8 种变体 |

最终从 `barley-logo-mr-optimized.html` 中选定 **Connected 徽章** 作为正式 Logo。
