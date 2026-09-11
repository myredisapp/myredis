#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Generate barley-logo-mr-optimized.html with 8 badge + 8 top variations."""

THEMES = [
    ("春分", "Spring", "#4caf50", "#7cb84b"),
    ("立夏", "Summer", "#22a6f2", "#6a9fd8"),
    ("初秋", "Golden", "#d99a1b", "#e5b33a"),
    ("深秋", "Autumn", "#f5b342", "#f9c75e"),
    ("冬至", "Winter", "#2563eb", "#60a5fa"),
]


def db_cylinder(c1, c2):
    """Return SVG paths for a database cylinder with two stripes."""
    return f'''
<ellipse cx="44" cy="26" rx="24" ry="9" fill="url(#{c1})"/>
<path d="M20 26 L20 70 Q20 78 44 78 Q68 78 68 70 L68 26" fill="url(#{c2})"/>
<path d="M20 42 Q44 50 68 42" stroke="rgba(255,255,255,0.35)" stroke-width="3" fill="none"/>
<path d="M20 60 Q44 68 68 60" stroke="rgba(255,255,255,0.35)" stroke-width="3" fill="none"/>
'''


def defs():
    grads = "\n".join(
        f'''<linearGradient id="g{i}" x1="0%" y1="0%" x2="0%" y2="100%">
<stop offset="0%" stop-color="{t[3]}"/>
<stop offset="100%" stop-color="{t[2]}"/>
</linearGradient>'''
        for i, t in enumerate(THEMES)
    )
    return f"""<svg width="0" height="0" style="position:absolute;">\n{grads}\n</svg>"""


BADGE_VARIATIONS = [
    (
        "经典徽章",
        "Classic",
        lambda i: f'''
{db_cylinder(f"g{i}", f"g{i}")}
<circle cx="72" cy="72" r="20" fill="#ffffff" stroke="url(#g{i})" stroke-width="4"/>
<text x="72" y="78" text-anchor="middle" font-family="'Noto Sans SC', sans-serif" font-weight="700" font-size="16" fill="url(#g{i})">MR</text>
'''
    ),
    (
        "六边形徽章",
        "Hex",
        lambda i: f'''
{db_cylinder(f"g{i}", f"g{i}")}
<path d="M72 50 L90 60 L90 84 L72 94 L54 84 L54 60 Z" fill="#ffffff" stroke="url(#g{i})" stroke-width="4"/>
<text x="72" y="78" text-anchor="middle" font-family="'Noto Sans SC', sans-serif" font-weight="700" font-size="14" fill="url(#g{i})">MR</text>
'''
    ),
    (
        "连接徽章",
        "Connected",
        lambda i: f'''
{db_cylinder(f"g{i}", f"g{i}")}
<circle cx="72" cy="72" r="18" fill="#ffffff" stroke="url(#g{i})" stroke-width="3"/>
<text x="72" y="77" text-anchor="middle" font-family="'Noto Sans SC', sans-serif" font-weight="700" font-size="13" fill="url(#g{i})">MR</text>
<circle cx="86" cy="56" r="5" fill="url(#g{i})"/>
<circle cx="56" cy="86" r="5" fill="url(#g{i})"/>
<path d="M68 56 L78 60 M64 78 L58 86" stroke="url(#g{i})" stroke-width="2" stroke-linecap="round"/>
'''
    ),
    (
        "麦粒徽章",
        "Grain",
        lambda i: f'''
{db_cylinder(f"g{i}", f"g{i}")}
<circle cx="72" cy="72" r="20" fill="#ffffff" stroke="url(#g{i})" stroke-width="4"/>
<text x="72" y="78" text-anchor="middle" font-family="'Noto Sans SC', sans-serif" font-weight="700" font-size="16" fill="url(#g{i})">MR</text>
<circle cx="72" cy="58" r="4" fill="url(#g{i})"/>
<circle cx="72" cy="86" r="4" fill="url(#g{i})"/>
'''
    ),
    (
        "分色徽章",
        "Split",
        lambda i: f'''
{db_cylinder(f"g{i}", f"g{i}")}
<circle cx="72" cy="72" r="20" fill="#ffffff" stroke="url(#g{i})" stroke-width="4"/>
<path d="M72 52 L72 92" stroke="url(#g{i})" stroke-width="2"/>
<text x="62" y="78" text-anchor="middle" font-family="'Noto Sans SC', sans-serif" font-weight="700" font-size="14" fill="url(#g{i})">M</text>
<text x="82" y="78" text-anchor="middle" font-family="'Noto Sans SC', sans-serif" font-weight="700" font-size="14" fill="url(#g{i})">R</text>
'''
    ),
    (
        "环形徽章",
        "Ring",
        lambda i: f'''
{db_cylinder(f"g{i}", f"g{i}")}
<path d="M52 72 A20 20 0 1 1 92 72" fill="none" stroke="url(#g{i})" stroke-width="5" stroke-linecap="round"/>
<text x="76" y="76" text-anchor="middle" font-family="'Noto Sans SC', sans-serif" font-weight="700" font-size="14" fill="url(#g{i})">MR</text>
'''
    ),
    (
        "方角徽章",
        "Square",
        lambda i: f'''
{db_cylinder(f"g{i}", f"g{i}")}
<rect x="52" y="52" width="40" height="40" rx="8" fill="#ffffff" stroke="url(#g{i})" stroke-width="4"/>
<text x="72" y="78" text-anchor="middle" font-family="'Noto Sans SC', sans-serif" font-weight="700" font-size="16" fill="url(#g{i})">MR</text>
'''
    ),
    (
        "菱形徽章",
        "Diamond",
        lambda i: f'''
{db_cylinder(f"g{i}", f"g{i}")}
<rect x="52" y="52" width="40" height="40" rx="6" fill="#ffffff" stroke="url(#g{i})" stroke-width="4" transform="rotate(45 72 72)"/>
<text x="72" y="78" text-anchor="middle" font-family="'Noto Sans SC', sans-serif" font-weight="700" font-size="14" fill="url(#g{i})">MR</text>
'''
    ),
]

TOP_VARIATIONS = [
    (
        "经典顶标",
        "Classic",
        lambda i: f'''
<ellipse cx="50" cy="28" rx="26" ry="10" fill="url(#g{i})"/>
<path d="M24 28 L24 72 Q24 82 50 82 Q76 82 76 72 L76 28" fill="url(#g{i})"/>
<path d="M24 46 Q50 56 76 46" stroke="rgba(255,255,255,0.35)" stroke-width="4" fill="none"/>
<path d="M24 64 Q50 74 76 64" stroke="rgba(255,255,255,0.35)" stroke-width="4" fill="none"/>
<text x="50" y="32" text-anchor="middle" font-family="'Noto Sans SC', sans-serif" font-weight="700" font-size="13" fill="#ffffff">MR</text>
'''
    ),
    (
        "顶标镂空",
        "Cutout",
        lambda i: f'''
<defs>
<mask id="cut{i}"><rect width="100" height="100" fill="white"/><text x="50" y="34" text-anchor="middle" font-family="'Noto Sans SC', sans-serif" font-weight="700" font-size="14" fill="black">MR</text></mask>
</defs>
<ellipse cx="50" cy="28" rx="26" ry="10" fill="url(#g{i})" mask="url(#cut{i})"/>
<path d="M24 28 L24 72 Q24 82 50 82 Q76 82 76 72 L76 28" fill="url(#g{i})"/>
<path d="M24 46 Q50 56 76 46" stroke="rgba(255,255,255,0.35)" stroke-width="4" fill="none"/>
<path d="M24 64 Q50 74 76 64" stroke="rgba(255,255,255,0.35)" stroke-width="4" fill="none"/>
'''
    ),
    (
        "丝带顶标",
        "Ribbon",
        lambda i: f'''
<ellipse cx="50" cy="28" rx="26" ry="10" fill="url(#g{i})"/>
<path d="M24 28 L24 72 Q24 82 50 82 Q76 82 76 72 L76 28" fill="url(#g{i})"/>
<path d="M24 46 Q50 56 76 46" stroke="rgba(255,255,255,0.35)" stroke-width="4" fill="none"/>
<path d="M24 64 Q50 74 76 64" stroke="rgba(255,255,255,0.35)" stroke-width="4" fill="none"/>
<path d="M18 18 L82 18 L76 36 L24 36 Z" fill="#ffffff"/>
<text x="50" y="32" text-anchor="middle" font-family="'Noto Sans SC', sans-serif" font-weight="700" font-size="13" fill="url(#g{i})">MR</text>
'''
    ),
    (
        "正面大标",
        "Front",
        lambda i: f'''
<ellipse cx="50" cy="24" rx="26" ry="9" fill="url(#g{i})"/>
<path d="M24 24 L24 74 Q24 84 50 84 Q76 84 76 74 L76 24" fill="url(#g{i})"/>
<path d="M24 42 Q50 52 76 42" stroke="rgba(255,255,255,0.35)" stroke-width="4" fill="none"/>
<path d="M24 64 Q50 74 76 64" stroke="rgba(255,255,255,0.35)" stroke-width="4" fill="none"/>
<rect x="30" y="44" width="40" height="22" rx="5" fill="rgba(255,255,255,0.95)"/>
<text x="50" y="61" text-anchor="middle" font-family="'Noto Sans SC', sans-serif" font-weight="700" font-size="16" fill="url(#g{i})">MR</text>
'''
    ),
    (
        "麦穗顶标",
        "Wheat",
        lambda i: f'''
<ellipse cx="50" cy="30" rx="24" ry="9" fill="url(#g{i})"/>
<path d="M26 30 L26 72 Q26 80 50 80 Q74 80 74 72 L74 30" fill="url(#g{i})"/>
<path d="M26 46 Q50 54 74 46" stroke="rgba(255,255,255,0.35)" stroke-width="3" fill="none"/>
<path d="M26 62 Q50 70 74 62" stroke="rgba(255,255,255,0.35)" stroke-width="3" fill="none"/>
<text x="50" y="34" text-anchor="middle" font-family="'Noto Sans SC', sans-serif" font-weight="700" font-size="12" fill="#ffffff">MR</text>
<path d="M14 40 Q18 30 14 20" stroke="url(#g{i})" stroke-width="4" fill="none" stroke-linecap="round"/>
<path d="M86 40 Q82 30 86 20" stroke="url(#g{i})" stroke-width="4" fill="none" stroke-linecap="round"/>
<circle cx="14" cy="30" r="4" fill="url(#g{i})"/>
<circle cx="86" cy="30" r="4" fill="url(#g{i})"/>
'''
    ),
    (
        "节点顶标",
        "Nodes",
        lambda i: f'''
<ellipse cx="50" cy="28" rx="24" ry="9" fill="url(#g{i})"/>
<path d="M26 28 L26 72 Q26 80 50 80 Q74 80 74 72 L74 28" fill="url(#g{i})"/>
<path d="M26 44 Q50 52 74 44" stroke="rgba(255,255,255,0.35)" stroke-width="3" fill="none"/>
<path d="M26 62 Q50 70 74 62" stroke="rgba(255,255,255,0.35)" stroke-width="3" fill="none"/>
<text x="50" y="32" text-anchor="middle" font-family="'Noto Sans SC', sans-serif" font-weight="700" font-size="12" fill="#ffffff">MR</text>
<circle cx="18" cy="72" r="5" fill="url(#g{i})"/>
<circle cx="82" cy="72" r="5" fill="url(#g{i})"/>
<circle cx="50" cy="90" r="5" fill="url(#g{i})"/>
<path d="M22 72 L26 68 M78 72 L74 68 M50 86 L50 80" stroke="url(#g{i})" stroke-width="2" stroke-linecap="round"/>
'''
    ),
    (
        "盾牌顶标",
        "Shield",
        lambda i: f'''
<ellipse cx="50" cy="28" rx="26" ry="10" fill="url(#g{i})"/>
<path d="M24 28 L24 72 Q24 82 50 82 Q76 82 76 72 L76 28" fill="url(#g{i})"/>
<path d="M24 46 Q50 56 76 46" stroke="rgba(255,255,255,0.35)" stroke-width="4" fill="none"/>
<path d="M24 64 Q50 74 76 64" stroke="rgba(255,255,255,0.35)" stroke-width="4" fill="none"/>
<path d="M38 14 L62 14 L62 28 Q62 38 50 44 Q38 38 38 28 Z" fill="#ffffff" stroke="url(#g{i})" stroke-width="3"/>
<text x="50" y="32" text-anchor="middle" font-family="'Noto Sans SC', sans-serif" font-weight="700" font-size="11" fill="url(#g{i})">MR</text>
'''
    ),
    (
        "极简顶标",
        "Minimal",
        lambda i: f'''
<ellipse cx="50" cy="28" rx="26" ry="10" fill="url(#g{i})"/>
<path d="M24 28 L24 70 Q24 80 50 80 Q76 80 76 70 L76 28" fill="url(#g{i})"/>
<path d="M24 48 Q50 56 76 48" stroke="rgba(255,255,255,0.4)" stroke-width="5" fill="none"/>
<path d="M24 66 Q50 74 76 66" stroke="rgba(255,255,255,0.4)" stroke-width="5" fill="none"/>
<text x="50" y="34" text-anchor="middle" font-family="'Noto Sans SC', sans-serif" font-weight="700" font-size="18" fill="#ffffff" letter-spacing="2">MR</text>
'''
    ),
]


CSS = """
  :root {
    --bg: #faf8f4; --surface: #ffffff; --text: #1f1a14;
    --text-muted: #7d6f5f; --border: #e7e0d5; --accent: #b8841f;
    --shadow: 0 14px 40px rgba(60, 45, 25, 0.10); --radius: 14px;
  }
  @media (prefers-color-scheme: dark) {
    :root:not([data-theme="light"]) {
      --bg: #16120d; --surface: #1e1913; --text: #f5e6d3;
      --text-muted: #a08b72; --border: #3d3226; --accent: #f5b342;
      --shadow: 0 18px 50px rgba(0, 0, 0, 0.45);
    }
  }
  :root[data-theme="dark"] {
    --bg: #16120d; --surface: #1e1913; --text: #f5e6d3;
    --text-muted: #a08b72; --border: #3d3226; --accent: #f5b342;
    --shadow: 0 18px 50px rgba(0, 0, 0, 0.55);
  }
  * { box-sizing: border-box; margin: 0; padding: 0; }
  body { font-family: "Noto Sans SC", "PingFang SC", "Microsoft YaHei", sans-serif; background: var(--bg); color: var(--text); line-height: 1.65; min-height: 100vh; }
  header { max-width: 1200px; margin: 0 auto; padding: 60px 24px 36px; text-align: center; }
  .eyebrow { display: inline-flex; align-items: center; gap: 8px; font-size: 12px; font-weight: 700; letter-spacing: 0.18em; text-transform: uppercase; color: var(--text-muted); margin-bottom: 16px; }
  .eyebrow::before, .eyebrow::after { content: ""; width: 28px; height: 1px; background: var(--border); }
  h1 { font-family: "Noto Serif SC", "Songti SC", serif; font-size: clamp(32px, 5vw, 50px); font-weight: 700; line-height: 1.2; margin-bottom: 16px; text-wrap: balance; }
  .subtitle { max-width: 640px; margin: 0 auto 32px; font-size: 16px; color: var(--text-muted); }
  .theme-strip { display: inline-flex; flex-wrap: wrap; gap: 10px 18px; justify-content: center; padding: 12px 20px; background: var(--surface); border: 1px solid var(--border); border-radius: 999px; box-shadow: var(--shadow); }
  .theme-chip { display: inline-flex; align-items: center; gap: 8px; font-size: 12px; color: var(--text-muted); }
  .theme-chip span { width: 14px; height: 14px; border-radius: 50%; border: 2px solid var(--surface); box-shadow: 0 0 0 1px var(--border); }
  main { max-width: 1200px; margin: 0 auto; padding: 0 24px 80px; }
  .section-title { font-family: "Noto Serif SC", serif; font-size: 28px; font-weight: 700; margin: 56px 0 28px; padding-bottom: 12px; border-bottom: 1px solid var(--border); }
  .section-title small { display: block; font-family: "Noto Sans SC", sans-serif; font-size: 14px; font-weight: 400; color: var(--text-muted); margin-top: 6px; }
  .grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(260px, 1fr)); gap: 20px; }
  .card { background: var(--surface); border: 1px solid var(--border); border-radius: var(--radius); padding: 18px; box-shadow: var(--shadow); transition: transform 0.2s ease; }
  .card:hover { transform: translateY(-3px); }
  .card-header { display: flex; align-items: center; justify-content: space-between; margin-bottom: 12px; }
  .card-title { font-size: 15px; font-weight: 700; }
  .card-sub { font-size: 11px; color: var(--text-muted); text-transform: uppercase; letter-spacing: 0.05em; }
  .main-mark { height: 110px; display: flex; align-items: center; justify-content: center; background: radial-gradient(circle at 50% 60%, rgba(0,0,0,0.03) 0%, transparent 70%); border-radius: 10px; margin-bottom: 12px; }
  .main-mark svg { width: 90px; height: 90px; filter: drop-shadow(0 8px 14px rgba(0,0,0,0.12)); }
  .color-row { display: flex; gap: 8px; justify-content: center; }
  .swatch { flex: 1; max-width: 48px; aspect-ratio: 1; border-radius: 8px; border: 1px solid var(--border); display: flex; align-items: center; justify-content: center; background: var(--bg); }
  .swatch svg { width: 34px; height: 34px; }
  .usage { max-width: 760px; margin: 60px auto 0; padding: 24px; background: var(--surface); border: 1px solid var(--border); border-radius: var(--radius); box-shadow: var(--shadow); }
  .usage h3 { font-family: "Noto Serif SC", serif; font-size: 18px; margin-bottom: 10px; }
  .usage p { color: var(--text-muted); font-size: 14px; }
  @media (max-width: 640px) { header { padding-top: 40px; } .grid { grid-template-columns: 1fr; } }
"""


def render_card(name, en, svg_fn):
    main_svg = f'<svg viewBox="0 0 100 100" xmlns="http://www.w3.org/2000/svg">{svg_fn(3).strip()}</svg>'
    swatches = "\n".join(
        f'<div class="swatch"><svg viewBox="0 0 100 100" xmlns="http://www.w3.org/2000/svg">{svg_fn(i).strip()}</svg></div>'
        for i in range(5)
    )
    return f'''
<div class="card">
  <div class="card-header">
    <span class="card-title">{name}</span>
    <span class="card-sub">{en}</span>
  </div>
  <div class="main-mark">{main_svg}</div>
  <div class="color-row">{swatches}</div>
</div>
'''


def render_section(title, subtitle, variations):
    cards = "\n".join(render_card(name, en, fn) for name, en, fn in variations)
    return f'''
<h2 class="section-title">{title}<small>{subtitle}</small></h2>
<div class="grid">{cards}</div>
'''


def main():
    badge_section = render_section("MR 徽章 · Badge", "8 种徽章形态优化，主预览为深秋默认色，下方为五季配色", BADGE_VARIATIONS)
    top_section = render_section("MR 顶标 · Top", "8 种顶标形态优化，主预览为深秋默认色，下方为五季配色", TOP_VARIATIONS)

    html = f"""<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>麦地 Logo MR 优化方案</title>
<link rel="preconnect" href="https://fonts.googleapis.com">
<link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
<link href="https://fonts.googleapis.com/css2?family=Noto+Sans+SC:wght@400;500;700&family=Noto+Serif+SC:wght@500;700&display=swap" rel="stylesheet">
<style>{CSS}</style>
</head>
<body>
{defs()}
<header>
  <div class="eyebrow">MaiDi Cache · MR Optimization</div>
  <h1>麦地缓存 · MR 徽章 / 顶标 优化方案</h1>
  <p class="subtitle">在你选定的两个方向上，各自延伸出 8 种形态变体。主图为深秋默认色，下方小图为四季配色。</p>
  <div class="theme-strip">
    <div class="theme-chip"><span style="background:#4caf50"></span>春分 · Spring</div>
    <div class="theme-chip"><span style="background:#22a6f2"></span>立夏 · Summer</div>
    <div class="theme-chip"><span style="background:#d99a1b"></span>初秋 · Golden</div>
    <div class="theme-chip"><span style="background:#f5b342"></span>深秋 · Autumn</div>
    <div class="theme-chip"><span style="background:#2563eb"></span>冬至 · Winter</div>
  </div>
</header>
<main>
  {badge_section}
  {top_section}
  <aside class="usage">
    <h3>使用建议</h3>
    <p>主预览采用项目默认主题“深秋”。如果某个变体符合预期，可以直接告诉我编号，我会导出对应 SVG 并替换进 frontend/index.html 标题栏。</p>
  </aside>
</main>
</body>
</html>"""

    with open("/Users/yanshili/me/projects/myredis/barley-logo-mr-optimized.html", "w", encoding="utf-8") as f:
        f.write(html)
    print("Generated: /Users/yanshili/me/projects/myredis/barley-logo-mr-optimized.html")


if __name__ == "__main__":
    main()
