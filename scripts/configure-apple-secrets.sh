#!/usr/bin/env bash
#
# 把 6 个 APPLE_* secret 配进仓库（DEVELOPMENT.md §2.5 D / §8.10）。
#
# 为什么必须 6 个一起配：.github/scripts/setup-macos-signing.sh 是**成组校验** ——
# 一份都没有 = 告警放行（接受未签名产物）；缺一份 = 直接失败；只签名不配公证也直接失败
# （只签不公证的包仍然过不了 Gatekeeper，等于没解决问题）。也就是说「半配」的唯一效果
# 是把下一次发版构建弄红，所以这里在写入之前先把 6 个值查齐，不给半配留机会。
#
# 默认是**演练**（DRY_RUN）：逐个走一遍 gh 的本地加密（`--no-store`，不会写进仓库），
# 把「值能被 gh 处理 + 这个 token 有权限」这条链路先验一遍，再让你显式放行写入。
#
# 值从哪来：scripts/prepare-apple-signing.sh 产出的凭据文件（默认
# ~/.tauri/myredis-apple-signing.env）—— 那个脚本已经用 openssl 把 .p12 解开、
# 与证书里的身份 / 团队 ID 对齐过，所以到这里不该再出「配错」这类问题。
#
# 用法：
#   bash scripts/configure-apple-secrets.sh                    # 演练（默认，不写入）
#   ALLOW_CONFIGURE=1 bash scripts/configure-apple-secrets.sh  # 真正写入（非交互）
#   bash scripts/configure-apple-secrets.sh                    # 交互：写入前问一次 yes
#     ENV_FILE=...      凭据文件（默认 ~/.tauri/myredis-apple-signing.env）
#     REPO=...          目标仓库（默认按当前目录的 gh remote 推导）
#     DRY_RUN=1         强制演练
#     VERIFY_ONLY=1     只验收：gh secret list 里这 6 个是否都在（D 的验收判据）
#   上面这些开关两种写法都认：写在命令前（`VERIFY_ONLY=1 bash ...`，推荐）或跟在脚本名后
#   （`bash ... VERIFY_ONLY=1`）—— 写反位置不应该是「静默没生效」。
#
# 值一律走标准输入传给 gh，**不用 `--body`**：命令行参数会出现在本机 ps 里
# （scripts/rotate-signing-key.sh 对私钥密码有同款考量）。脚本自身也从不回显任何值，
# 只打印名字、长度与时间戳。
#
# 写法上刻意只用 bash 3.2 就有的东西（macOS 自带的是 3.2，没有 `declare -A`）——
# 与 scripts/ 下其它脚本一致。
set -euo pipefail

die() { echo "✗ $*" >&2; exit 1; }
ok() { echo "  ✓ $*"; }
warn() { echo "  ! $*" >&2; }

# 开关写成 `NAME=值` 的环境变量前缀（推荐），但也认「跟在脚本名后面」的写法
# （`bash scripts/configure-apple-secrets.sh VERIFY_ONLY=1`）—— 后者很容易被顺手写出来，
# 而不认它的后果是「静默跑成了演练」或「明明放行了却没写入」，不如在这里收下。
for arg in "$@"; do
  case "${arg}" in
    VERIFY_ONLY=*|DRY_RUN=*|ALLOW_CONFIGURE=*|ENV_FILE=*|REPO=*) export "${arg}" ;;
    *) die "不认识的参数：${arg}（用法见本脚本头部注释）" ;;
  esac
done

ENV_FILE="${ENV_FILE:-$HOME/.tauri/myredis-apple-signing.env}"
SECRETS=(APPLE_CERTIFICATE APPLE_CERTIFICATE_PASSWORD APPLE_SIGNING_IDENTITY APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID)

command -v gh >/dev/null 2>&1 || die "找不到 gh（GitHub CLI）。装一个：brew install gh，然后 gh auth login"

# ---------------------------------------------------------------------------
# 1. 目标仓库：默认跟着当前仓库走，避免手抄仓库名抄错
# ---------------------------------------------------------------------------
if [ -n "${REPO:-}" ]; then
  REPO_LABEL="${REPO}"
else
  REPO_LABEL="$(gh repo view --json nameWithOwner -q .nameWithOwner 2>/dev/null)" \
    || die "推导不出目标仓库（gh 没登录 / 当前目录不是仓库 / remote 不认识）。要么先 gh auth login，要么显式给 REPO=<owner/name>"
  REPO="${REPO_LABEL}"
fi
gh auth status >/dev/null 2>&1 || die "gh 未登录：先 gh auth login（配 secret 需要该仓库的 admin 权限）"

# ---------------------------------------------------------------------------
# 2. 读凭据文件：6 个值必须齐（半配会把下次发版构建弄红，所以在这里就拦下）
# ---------------------------------------------------------------------------
# 只认「名字=值」这一种行（注释与空行跳过）；值里的 = 不会被截断（base64 的补位要留着）
value_of() {
  local want="$1" line key value
  while IFS= read -r line || [ -n "${line}" ]; do
    case "${line}" in ''|'#'*) continue ;; esac
    key="${line%%=*}"
    [ "${key}" = "${want}" ] || continue
    value="${line#*=}"
    # 去掉可能存在的成对引号（dotenv 风格）
    case "${value}" in
      \"*\") value="${value#\"}"; value="${value%\"}" ;;
      \'*\') value="${value#\'}"; value="${value%\'}" ;;
    esac
    printf '%s' "${value}"
    return 0
  done < "${ENV_FILE}"
  return 1
}

list_names() { gh secret list --repo "${REPO}" --json name -q '.[].name' 2>/dev/null; }

# 验收判据（D 的验收条件）：gh secret list 里 6 个都在
verify() {
  local present missing_lines="" name count=0
  present="$(list_names)" || die "读不出 secret 列表（权限不够？需要仓库 admin）：gh secret list --repo ${REPO}"
  for name in "${SECRETS[@]}"; do
    if printf '%s\n' "${present}" | grep -qx "${name}"; then
      count=$((count + 1))
    else
      missing_lines="${missing_lines}${name} "
    fi
  done
  echo "• 验收：gh secret list --repo ${REPO}"
  gh secret list --repo "${REPO}"
  if [ "${count}" -eq "${#SECRETS[@]}" ]; then
    echo "✓ 6 个 APPLE_* secret 都在（D 的验收判据达成）"
    return 0
  fi
  echo "✗ 只找到 ${count}/6 个，还缺：${missing_lines}" >&2
  return 1
}

if [ "${VERIFY_ONLY:-0}" = "1" ]; then
  verify
  exit $?
fi

[ -f "${ENV_FILE}" ] || die "找不到凭据文件 ${ENV_FILE}
   先生成它：bash scripts/prepare-apple-signing.sh（见 §2.5 C）"
# 600 是对这份文件的要求（里面有私钥与密码）：权限太松就先提醒。
# 两种 stat 的写法：BSD（macOS）用 -f '%Lp'，GNU 用 -c '%a'；GNU 的 `stat -f` 是「文件系统」
# 而不是格式，会吐出别的东西，所以只认「纯八进制」的输出，其余一律跳过提示
perm="$(stat -f '%Lp' "${ENV_FILE}" 2>/dev/null || stat -c '%a' "${ENV_FILE}" 2>/dev/null || true)"
case "${perm}" in
  ''|*[!0-7]*) ;;
  600) ;;
  *) warn "${ENV_FILE} 的权限是 ${perm}，建议 chmod 600（里面有私钥与密码）" ;;
esac

missing_count=0
missing_lines=""
for name in "${SECRETS[@]}"; do
  if ! value="$(value_of "${name}")" || [ -z "${value}" ]; then
    missing_count=$((missing_count + 1))
    missing_lines="${missing_lines}${name} "
  fi
done
[ "${missing_count}" -eq 0 ] || die "凭据文件里缺 ${missing_count} 个值：${missing_lines}
   ${ENV_FILE}
   6 个必须成组出现（缺一个就会把下次发版构建弄红），请补齐或重新跑 prepare-apple-signing.sh。"

ok "6 个值都读到了（只报长度与形状，不显示内容）"

# ---------------------------------------------------------------------------
# 3. 形状检查：这些错误在 CI 里的表现分别是「tauri 解不出证书」「签名身份挑不中」
#    「公证失败」，都发生在构建后半程，不如在这里当场拦下
# ---------------------------------------------------------------------------
cert_b64="$(value_of APPLE_CERTIFICATE | tr -d '[:space:]')"
[ "${cert_b64}" = "$(value_of APPLE_CERTIFICATE)" ] \
  || warn "APPLE_CERTIFICATE 里含空白（多行粘贴？）—— 已去掉空白再配（CI 里也有一道同样的兜底）"
printf '%s' "${cert_b64}" | grep -qE '^[A-Za-z0-9+/]+=*$' \
  || die "APPLE_CERTIFICATE 不是 base64（只应有 A-Za-z0-9+/ 与结尾的 =）—— 是不是把 .p12 路径或 PEM 文本当内容填了？"
[ "$(printf '%s' "${cert_b64}" | base64 -d 2>/dev/null | wc -c | tr -d ' ')" -gt 0 ] \
  || die "APPLE_CERTIFICATE 解不回二进制（base64 内容损坏）"

identity="$(value_of APPLE_SIGNING_IDENTITY)"
team_id="$(value_of APPLE_TEAM_ID)"
apple_id="$(value_of APPLE_ID)"
apple_pw="$(value_of APPLE_PASSWORD)"
cert_pw="$(value_of APPLE_CERTIFICATE_PASSWORD)"

printf '%s' "${identity}" | grep -q '^Developer ID Application: ' \
  || die "APPLE_SIGNING_IDENTITY 不是 Developer ID Application 身份：「${identity}」"
printf '%s' "${team_id}" | grep -qE '^[A-Z0-9]{10}$' \
  || die "APPLE_TEAM_ID 看起来不是 10 位团队 ID：「${team_id}」"
printf '%s' "${apple_id}" | grep -q '@' \
  || warn "APPLE_ID 不像邮箱：「${apple_id}」"
printf '%s' "${apple_pw}" | grep -qE '^[a-z]{4}-[a-z]{4}-[a-z]{4}-[a-z]{4}$' \
  || warn "APPLE_PASSWORD 不像 App 专用密码（形如 abcd-efgh-ijkl-mnop）—— 若公证失败先怀疑这个"

for name in "${SECRETS[@]}"; do
  printf '    %-28s %5d 字节\n' "${name}" "$(value_of "${name}" | wc -c | tr -d ' ')"
done
echo "    身份：${identity}   团队：${team_id}   证书密码：${#cert_pw} 字符"

# ---------------------------------------------------------------------------
# 4. 演练（默认）：逐个走一遍 gh 的本地加密，但不写入仓库
# ---------------------------------------------------------------------------
# `--no-store` 会取仓库公钥、把值本地加密后**打印密文而不上传** ——
# 正好用来验「值能被 gh 正常处理 + 这个 token 有权限读仓库公钥」这条链路。
# 跑完再比对一次 secret 列表，确认仓库状态没被动过。
dry_run() {
  local held_before held_after name
  held_before="$(list_names)" || die "演练都读不到 secret 列表：gh secret list --repo ${REPO}"
  echo "• 演练：逐个加密（--no-store，不写入 ${REPO}）"
  for name in "${SECRETS[@]}"; do
    value_of "${name}" | gh secret set "${name}" --repo "${REPO}" --no-store >/dev/null \
      || die "『${name}』加密失败（gh 的报错见上）—— 先解决它，别急着写入"
    ok "${name} 加密通过（未写入）"
  done
  held_after="$(list_names)"
  if [ "${held_before}" = "${held_after}" ]; then
    echo "✓ 演练结束：6 个值都能正常加密，且 ${REPO} 的 secret 列表未发生任何变化"
  else
    die "演练前后 secret 列表不一致 —— 这不是演练该有的行为，请检查 gh 版本"
  fi
}

if [ "${DRY_RUN:-1}" = "1" ]; then
  dry_run
  cat <<EOF

• 下一步：确认无误后真正写入（会覆盖这 6 个 secret 的现有值）：
    ALLOW_CONFIGURE=1 bash scripts/configure-apple-secrets.sh
  写入后的验收（等价于本脚本的 VERIFY_ONLY 模式）：
    bash scripts/configure-apple-secrets.sh VERIFY_ONLY=1
EOF
  exit 0
fi

# ---------------------------------------------------------------------------
# 5. 真正写入：先要一次明确放行，再逐个经 stdin 传入
# ---------------------------------------------------------------------------
echo
echo "• 即将把下面 6 个值写入 ${REPO}（同名 secret 会被覆盖）："
printf '    %s\n' "${SECRETS[@]}"
if [ "${ALLOW_CONFIGURE:-0}" != "1" ]; then
  [ -t 0 ] || die "非交互路径必须显式放行：ALLOW_CONFIGURE=1 bash scripts/configure-apple-secrets.sh"
  printf '确认写入？输入 yes 继续: ' >&2
  IFS= read -r answer
  [ "${answer}" = "yes" ] || die "已取消（没有写入任何 secret）"
fi

for name in "${SECRETS[@]}"; do
  value_of "${name}" | gh secret set "${name}" --repo "${REPO}" >/dev/null \
    || die "写入『${name}』失败。前面几个可能已经写进去了 —— 重新跑本脚本会把这 6 个整体覆盖一遍，不会留下半配状态"
  ok "${name} 已写入"
done

echo
verify || die "写入完成但验收没通过 —— 见上面的列表"
cat <<EOF

• 下一步（§2.5 E）：用预发布 tag 跑一遍真实流水线，让「导入证书 → 签名 → 公证 → staple」
  与构建后的复核（verify-macos-signing.sh）在 CI 上真正执行：
    git tag v0.0.17-beta.1 && git push origin v0.0.17-beta.1
  （带 `-` 的 tag 会被 release.yml 当预发布，不占 latest 别名，不影响已装用户）

• 收尾：确认流水线全绿后，把这份凭据文件从工作机挪到离线介质
  （${ENV_FILE}），别留在随手能读到的地方。
EOF
