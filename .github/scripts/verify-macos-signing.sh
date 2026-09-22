#!/usr/bin/env bash
#
# 复核 macOS 产物的签名与公证（DEVELOPMENT.md §2.4 / §4 #6）。
#
# 为什么要有这一步：tauri 在凭据缺失时**静默不签名**，构建照样成功 ——
# 问题只会在用户下载后表现为「首次打开被 Gatekeeper 拦下」。凭据配了就必须验到，
# 让「配置错了」这种事在发版流水线里当场失败。
#
# 用法：verify-macos-signing.sh <bundle 目录>
#   bundle 目录 = src-tauri/target/<target>/release/bundle
#   未配凭据（MACOS_SIGNING_ENABLED != 1）时只打一条告警，不拦流水线。
set -euo pipefail

BUNDLE_DIR="${1:?用法: verify-macos-signing.sh <bundle 目录>}"

APP="$(find "$BUNDLE_DIR/macos" -maxdepth 1 -name '*.app' -print -quit 2>/dev/null || true)"
if [ -z "$APP" ]; then
  echo "::error::没在 $BUNDLE_DIR/macos 下找到 .app 产物" >&2
  exit 1
fi

if [ "${MACOS_SIGNING_ENABLED:-}" != "1" ]; then
  echo "::warning::未配置签名 / 公证凭据，本次产物（${APP}）未签名：用户首次打开需右键「打开」，且不会通过 Gatekeeper 的下载校验"
  exit 0
fi

fail() { echo "::error::$*" >&2; exit 1; }

echo "• 校验签名：$APP"
codesign --verify --deep --strict --verbose=2 "$APP" \
  || fail "codesign 校验失败：产物没有有效签名（凭据配了却没签上）"

# 必须是指定的 Developer ID 身份签的：ad-hoc 签名也能过 codesign --verify，
# 但它对 Gatekeeper / 公证毫无用处
SIGN_INFO="$(mktemp)"
codesign -dv --verbose=4 "$APP" 2>"$SIGN_INFO" || true
cat "$SIGN_INFO"
grep -q "Authority=Developer ID Application" "$SIGN_INFO" \
  || fail "签名身份不是 Developer ID Application（见上面的 codesign 输出）"
grep -q "TeamIdentifier=${APPLE_TEAM_ID:-}" "$SIGN_INFO" \
  || fail "产物里的 TeamIdentifier 与 APPLE_TEAM_ID（${APPLE_TEAM_ID:-未设置}）不一致"

# spctl 就是 Gatekeeper 的判据：签名有效但**没公证**时它会拒绝 ——
# 正是我们要拦下的那种「自己机器能装、用户装不上」
echo "• Gatekeeper 评估（spctl）"
spctl -a -vvv -t exec "$APP" \
  || fail "Gatekeeper 校验未通过（多半是没公证，或证书链不完整）"

# 票据必须已 staple 进产物：用户离线首次打开时也靠它过 Gatekeeper
echo "• 公证票据（stapler）"
xcrun stapler validate "$APP"

# dmg 是用户实际下载的文件，单独验一遍：
# 判据用 spctl 的 disk image 形式（open + primary-signature），与 Finder 打开时一致
DMG="$(find "$BUNDLE_DIR/dmg" -maxdepth 1 -name '*.dmg' -print -quit 2>/dev/null || true)"
if [ -n "$DMG" ]; then
  echo "• 校验安装包：$DMG"
  codesign --verify --verbose=2 "$DMG"
  spctl -a -t open --context context:primary-signature -v "$DMG" \
    || fail "dmg 的 Gatekeeper 校验未通过"
  # dmg 有没有被 staple 不在我们的控制范围内（取决于 tauri 版本），
  # 因此只告警：里面的 .app 已验过签名 + 票据，用户拖出来照样能开
  xcrun stapler validate "$DMG" \
    || echo "::warning::dmg 上没有 staple 的公证票据（.app 已验过，一般不影响使用）"
fi

echo "✓ macOS 签名与公证校验通过：$(basename "$APP")"
