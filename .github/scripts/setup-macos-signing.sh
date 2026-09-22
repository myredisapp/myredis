#!/usr/bin/env bash
#
# 配置 macOS 代码签名与公证凭据（DEVELOPMENT.md §2.4 / §4 #6）。
#
# 为什么需要它：不签名 + 不公证的 .app 首次打开会被 Gatekeeper 拦下（只能右键「打开」），
# 而配错的凭据**不会**让构建失败 —— 产物照样出，只是悄悄没签名。所以这里在构建前
# 把凭据校验一遍：要么一份都没有（明确告知产物未签名），要么必须成组配齐。
#
# 凭据（GitHub secrets，全部可选，但必须成组出现）：
#
#   签名：
#     APPLE_CERTIFICATE            Developer ID Application 证书的 .p12 **base64 内容**
#                                  （macOS: `base64 -i cert.p12`，输出应为单行）
#     APPLE_CERTIFICATE_PASSWORD   .p12 的导出密码
#     APPLE_SIGNING_IDENTITY       签名身份，如 `Developer ID Application: 麦地 (ABCDE12345)`
#
#   公证（Apple ID 方式，任选其一；API Key 方式见 tauri 文档）：
#     APPLE_ID                     Apple 账号
#     APPLE_PASSWORD               该账号的 App 专用密码（不是登录密码）
#     APPLE_TEAM_ID                团队 ID
#
# 配齐后 tauri build 自己完成「导入证书 → 签名 → 公证 → staple 票据」，无需额外步骤；
# 本脚本只负责校验并写进 $GITHUB_ENV，构建后的复核见 release.yml 的「校验 macOS 签名与公证」。
set -euo pipefail

: "${GITHUB_ENV:?本脚本要在 GitHub Actions 里运行（需要 GITHUB_ENV）}"

# 只签不公证仍然会被 Gatekeeper 拦（用户得右键打开），等于没解决问题：
# 两组凭据要么都不给（接受未签名产物），要么都给
SIGN_KEYS=(APPLE_CERTIFICATE APPLE_CERTIFICATE_PASSWORD APPLE_SIGNING_IDENTITY)
NOTARIZE_KEYS=(APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID)

missing_of() {
  local missing=()
  for key in "$@"; do
    [ -n "${!key:-}" ] || missing+=("$key")
  done
  printf '%s' "${missing[*]:-}"
}

count_missing() {
  local count=0
  for key in "$@"; do
    [ -n "${!key:-}" ] || count=$((count + 1))
  done
  printf '%s' "$count"
}

sign_missing="$(missing_of "${SIGN_KEYS[@]}")"
notarize_missing="$(missing_of "${NOTARIZE_KEYS[@]}")"

# 一份都没有：保持现状（未签名产物 + 明确告警，而不是让人以为签过了）
if [ "$(count_missing "${SIGN_KEYS[@]}")" = "3" ] && [ "$(count_missing "${NOTARIZE_KEYS[@]}")" = "3" ]; then
  echo "::warning::macOS 签名 / 公证凭据未配置：本次产物未签名，用户首次打开需右键「打开」（见 DEVELOPMENT.md §4 #6）"
  exit 0
fi

if [ -n "$sign_missing" ]; then
  echo "::error::macOS 签名凭据不完整，缺少: ${sign_missing}（三个必须一起配，见 .github/scripts/setup-macos-signing.sh 注释）" >&2
  exit 1
fi

if [ -n "$notarize_missing" ]; then
  echo "::error::已配置签名凭据，但公证凭据不完整，缺少: ${notarize_missing}。只签名不公证的产物仍会被 Gatekeeper 拦下（用户需右键打开），等于没解决问题；请补齐这三项或把签名凭据一并移除" >&2
  exit 1
fi

# base64 里混入换行会让 tauri 解不出证书（macOS 的 `base64 -i` 是单行输出，
# 但从别处粘贴时容易带上换行）：这里去掉空白并提示一次，而不是让构建去报解码错误
cert_normalized="$(printf '%s' "$APPLE_CERTIFICATE" | tr -d '[:space:]')"
if [ "$cert_normalized" != "$APPLE_CERTIFICATE" ]; then
  echo "::warning::APPLE_CERTIFICATE 里含空白字符，已自动去掉（建议用 \`base64 -i cert.p12\` 生成单行内容）"
fi

# 写入 $GITHUB_ENV，后续步骤（tauri build 与签名复核）都能读到。
# 用 heredoc 形式写入：值里即便有特殊字符也不会被 shell 二次解释
{
  printf 'APPLE_CERTIFICATE<<__APPLE_CERT_EOF__\n%s\n__APPLE_CERT_EOF__\n' "$cert_normalized"
  printf 'APPLE_CERTIFICATE_PASSWORD<<__APPLE_PW_EOF__\n%s\n__APPLE_PW_EOF__\n' "$APPLE_CERTIFICATE_PASSWORD"
  printf 'APPLE_SIGNING_IDENTITY<<__APPLE_ID_EOF__\n%s\n__APPLE_ID_EOF__\n' "$APPLE_SIGNING_IDENTITY"
  printf 'APPLE_ID<<__APPLE_APPLEID_EOF__\n%s\n__APPLE_APPLEID_EOF__\n' "$APPLE_ID"
  printf 'APPLE_PASSWORD<<__APPLE_APPPW_EOF__\n%s\n__APPLE_APPPW_EOF__\n' "$APPLE_PASSWORD"
  printf 'APPLE_TEAM_ID<<__APPLE_TEAM_EOF__\n%s\n__APPLE_TEAM_EOF__\n' "$APPLE_TEAM_ID"
  # 复核步骤据此判断「该验签」还是「只提示未签名」
  echo "MACOS_SIGNING_ENABLED=1"
} >> "$GITHUB_ENV"

echo "• macOS 签名与公证已启用：身份「${APPLE_SIGNING_IDENTITY}」，团队 ${APPLE_TEAM_ID}"
echo "  tauri build 会自动完成导入证书 → 签名 → 公证 → staple，构建后由「校验 macOS 签名与公证」复核"
