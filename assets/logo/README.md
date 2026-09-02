# 麦地缓存 Logo

本目录存放麦地缓存（MaiDi Cache）的品牌标识源文件。

## 文件说明

| 文件 | 用途 |
|---|---|
| `logo.svg` | 主 Logo 源文件，使用 CSS 变量控制颜色，可直接编辑 |
| `logo-themed.svg` | 主题自适应版本，会跟随 `web/index.html` 的 `--accent` 变量自动变色 |
| `favicon.svg` | 浏览器标签页图标源文件，小尺寸优化 |
| `icon-source.svg` | Tauri 桌面应用图标源文件，固定深秋色，用于生成所有平台图标 |

## 如何修改颜色

### 方法 1：修改 SVG 文件内的 CSS 变量

打开 `logo.svg`，找到顶部的 `<style>`：

```css
:root {
  --md-logo-primary: #f5b342;
  --md-logo-secondary: #f9c75e;
  --md-logo-surface: #ffffff;
  --md-logo-stripe: rgba(255, 255, 255, 0.35);
}
```

直接修改这 4 个色值即可，整个 Logo 会同步更新。

### 方法 2：通过外部 CSS 覆盖

当 `logo.svg` 以内联方式（直接写入 HTML）使用时，可以在页面 CSS 中覆盖：

```css
:root {
  --md-logo-primary: #4caf50;
  --md-logo-secondary: #7cb84b;
}
```

### 方法 3：在矢量软件中编辑

用 Figma、Sketch、Adobe Illustrator 等软件直接打开 `.svg` 文件：
- 所有图形都是矢量路径，可任意缩放。
- 颜色填充已经分组，可直接选中并改色。

## 如何替换标题栏图标

项目标题栏图标位于 `web/index.html` 的 `.titlebar .logo i` 元素。

当前已替换为内联 SVG。如需换回 Font Awesome 图标，把：

```html
<svg class="logo-mark" viewBox="0 0 100 100" ...>...</svg>
```

改回：

```html
<i class="fas fa-crown"></i>
```

即可。

## 生成不同尺寸

### Tauri 桌面应用图标

本项目使用 Tauri 构建桌面应用。运行以下命令，会根据 `icon-source.svg` 重新生成 `src-tauri/icons/` 下的所有图标：

```bash
cargo tauri icon assets/logo/icon-source.svg
```

生成的文件包括：
- Windows: `icon.ico`
- macOS: `icon.icns`
- Linux: `icon.png`
- 通用尺寸: `32x32.png`, `64x64.png`, `128x128.png`, `128x128@2x.png`
- 移动端（如启用）: `ios/`, `android/`

`icon-source.svg` 使用固定深秋主题色，确保在各种平台渲染器下颜色一致。

### PNG / ICO（手动方式）

用浏览器打开 `logo.svg` 或 `favicon.svg`，按 `Cmd/Ctrl + Shift + P` 打开打印/导出，
或在设计软件中导出为：

- `icon.png` 512×512（应用商店）
- `icon@2x.png` 256×256（Retina）
- `favicon.ico` 16×16 / 32×32 / 48×48（浏览器标签）

### 命令行方式（需安装 ImageMagick）

```bash
magick -background none assets/logo/favicon.svg -resize 32x32 assets/logo/favicon.ico
```

## 设计归档

早期探索稿已移动到 `design/` 目录，仅供参考，不作为最终产物。
