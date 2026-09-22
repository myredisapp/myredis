#!/usr/bin/env bash
#
# 准备 macOS 代码签名 / 公证凭据并就地自检（DEVELOPMENT.md §2.5 C / §8.10）。
#
# 为什么需要它：要配的 6 个值里，有 4 类错误**在本地不会有任何反馈**，只会在 CI 里
# 以「构建跑完才发现」的形式暴露（打一个 tag 的成本远高于在本地把 .p12 解开看一眼）：
#
#   · 导出证书时没勾私钥（只拿到 .cer，或一个没有私钥的 .p12）→ 签不了名
#   · 用了 Apple Development / Mac Developer / Developer ID **Installer** 证书
#     —— 只有 Developer ID Application 能签分发给用户的 .app
#   · APPLE_SIGNING_IDENTITY 与证书里实际的 CN 不一致（团队名、大小写、括号）
#   · APPLE_PASSWORD 填了账号登录密码，而不是 App 专用密码（公证时才会报错）
#
# 现有两道 CI 检查都拦不住这些：setup-macos-signing.sh 只查「有没有配」，
# verify-macos-signing.sh 要等整个发版构建跑完才拦（见 §8.10）。所以这里把判据前移 ——
# 用 openssl 把 .p12 真解开，从证书本身读出签名身份（CN）与团队 ID（OU），
# 与要填的值逐个对照，最后产出 D 要用的 dotenv 文件（§2.5 D 的输入）。
#
# 前置：Apple Developer Program 会员（付费）+ 一份 Developer ID Application 证书。
# 还没有的话（本脚本会在第一步就告诉你，不会白跑后面）：
#   1. developer.apple.com → Certificates, Identifiers & Profiles → Certificates
#   2. 「+」→ Software → **Developer ID Application**（不是 Apple Development，
#      也不是 Developer ID Installer）→ 按提示用钥匙串生成 CSR 并上传
#   3. 下载 .cer 后双击导入钥匙串，在「钥匙串访问」里找到这张证书，
#      展开确认**带一把私钥**，然后「导出…」成 .p12，设一个导出密码（就是 P12_PASSWORD）
#   4. App 专用密码另去 appleid.apple.com → 登录与安全 → App 专用密码 生成
#      （形如 abcd-efgh-ijkl-mnop，**不是** Apple ID 的登录密码）
#
# 用法：
#   bash scripts/prepare-apple-signing.sh
#     P12_PATH=~/Desktop/developerID.p12   导出的证书（.p12，含私钥）
#     P12_PASSWORD=...                     导出 .p12 时设的密码（不给则交互输入，不回显）
#     APPLE_ID=you@example.com             公证用的 Apple 账号
#     APPLE_PASSWORD=...                   该账号的 App 专用密码（不给则交互输入，不回显）
#     APPLE_TEAM_ID=ABCDE12345             团队 ID（10 位）；不给则从证书的 OU 里读
#     APPLE_SIGNING_IDENTITY=...           签名身份；不给则从证书的 CN 里读（一般不用手填）
#     OUT=...                              产出文件（默认 ~/.tauri/myredis-apple-signing.env）
#     NO_WRITE=1                           只校验，不写产出文件
#     ALLOW_PASSWORD_FORMAT=1              放行「不像 App 专用密码」的 APPLE_PASSWORD
#
#   上面这些开关两种写法都认：写在命令前（`APPLE_ID=... bash ...`，推荐）或跟在脚本名后
#   （`bash ... APPLE_ID=...`）—— 写反位置不应该是「静默没生效」。
#
# 密码走环境变量还是交互输入：命令行参数会进 shell 历史，所以本脚本不提供
# `--password xxx` 这种形式；openssl 调用一律用 `-passin env:` 而不是 `pass:`
# （后者会让密码出现在本机 ps 里，见 scripts/rotate-signing-key.sh 的同款考量）。
set -euo pipefail

die() { echo "✗ $*" >&2; exit 1; }
ok() { echo "  ✓ $*"; }
warn() { echo "  ! $*" >&2; }

# 开关写成 `NAME=值` 的环境变量前缀（推荐），但也认「跟在脚本名后面」的写法 ——
# 顺手写反位置时不必猜为什么没生效（与 scripts/configure-apple-secrets.sh 同款处理）
for arg in "$@"; do
  case "${arg}" in
    P12_PATH=*|P12_PASSWORD=*|APPLE_ID=*|APPLE_PASSWORD=*|APPLE_TEAM_ID=*|APPLE_SIGNING_IDENTITY=*|OUT=*|NO_WRITE=*|ALLOW_PASSWORD_FORMAT=*)
      export "${arg}" ;;
    *) die "不认识的参数：${arg}（用法见本脚本头部注释）" ;;
  esac
done

P12="${P12_PATH:-}"
OUT="${OUT:-${HOME}/.tauri/myredis-apple-signing.env}"
IDENTITY_PREFIX="Developer ID Application: "

[ -n "${P12}" ] || die "请用 P12_PATH=<证书 .p12 的路径> 指定导出的 Developer ID Application 证书（见本脚本头部的前置说明）"
[ -f "${P12}" ] || die "找不到证书文件 ${P12}（P12_PATH 指错了？）"
command -v openssl >/dev/null 2>&1 || die "找不到 openssl"

# ---------------------------------------------------------------------------
# 1. 取密码：优先环境变量，其次交互输入（不回显，不进历史）
# ---------------------------------------------------------------------------
if [ -z "${P12_PASSWORD:-}" ]; then
  [ -t 0 ] || die "没有 P12_PASSWORD 环境变量，且当前不是交互终端 —— 无法询问 .p12 的导出密码"
  printf '请输入 %s 的导出密码（不回显）: ' "$(basename "${P12}")" >&2
  IFS= read -rs P12_PASSWORD; echo >&2
  [ -n "${P12_PASSWORD}" ] || die "密码为空"
fi

if [ -z "${APPLE_PASSWORD:-}" ]; then
  [ -t 0 ] || die "没有 APPLE_PASSWORD 环境变量，且当前不是交互终端 —— 无法询问 App 专用密码"
  printf '请输入 App 专用密码（appleid.apple.com 单独生成的那种，不回显）: ' >&2
  IFS= read -rs APPLE_PASSWORD; echo >&2
fi

# ---------------------------------------------------------------------------
# 2. 解开 .p12：先验密码，再确认里面**有私钥**
# ---------------------------------------------------------------------------
# 直接用 `-passin env:` 让 openssl 从环境读密码；`-info -noout` 只打印内部的
# bag 结构（能看出有没有私钥）而不导出任何内容 —— 密码不对时它会报
# "Mac verify error: invalid password?"，这正是我们要的第一个判据。
# 老一点的工具导出的 .p12 用 RC2 加密，OpenSSL 3 需要 -legacy 才解得开，
# 所以失败时按「有没有 -legacy 这个开关」再试一次，而不是让人对着报错猜。
p12_info() {
  local output status
  output="$(P12_PASSWORD="${P12_PASSWORD}" openssl pkcs12 -in "${P12}" -passin env:P12_PASSWORD -info -noout 2>&1)" && { printf '%s' "${output}"; return 0; }
  status=$?
  if printf '%s' "${output}" | grep -qi "invalid password\|mac verify error"; then
    printf '%s' "${output}"
    return 2
  fi
  if openssl pkcs12 -help 2>&1 | grep -q -- "-legacy"; then
    output="$(P12_PASSWORD="${P12_PASSWORD}" openssl pkcs12 -in "${P12}" -passin env:P12_PASSWORD -info -noout -legacy 2>&1)" \
      && { printf '%s' "${output}"; return 0; }
    if printf '%s' "${output}" | grep -qi "invalid password\|mac verify error"; then
      printf '%s' "${output}"
      return 2
    fi
  fi
  printf '%s' "${output}"
  return 1
}

if info="$(p12_info)"; then
  p12_status=0
else
  p12_status=$?
fi
case "${p12_status}" in
  0) ok "证书已用给定的密码解开（$(basename "${P12}")）" ;;
  2) die ".p12 的导出密码不对（openssl 报 Mac verify error）：${P12}" ;;
  *) die "解不开 ${P12}，openssl 说：
${info}
如果这份 .p12 是旧工具导出的（RC2 加密），需要 OpenSSL 3 的 -legacy（脚本已自动试过）；
否则多半不是一份合法的 PKCS#12 文件（比如误把 .cer 改了扩展名）。" ;;
esac

printf '%s' "${info}" | grep -qiE "shrouded keybag|key bag" \
  || die "这份 .p12 里只有证书、没有私钥：导出时没勾上私钥（或只导出了 .cer）。
   签名必须用带私钥的那一份：在「钥匙串访问」里找到 Developer ID Application 证书，
   展开确认它下面挂着一个私钥，再「导出…」成 .p12。"
ok "证书里带私钥（可以签名）"

# 取出那张叶子证书（-nokeys -clcerts 只读证书，不碰私钥，因此两个 openssl 分支都不用 -noenc/-nodes）
cert_pem() {
  P12_PASSWORD="${P12_PASSWORD}" openssl pkcs12 -in "${P12}" -passin env:P12_PASSWORD -nokeys -clcerts 2>/dev/null \
    || P12_PASSWORD="${P12_PASSWORD}" openssl pkcs12 -in "${P12}" -passin env:P12_PASSWORD -nokeys -clcerts -legacy 2>/dev/null
}

CERT_TMP="$(mktemp)"
cleanup() { rm -f "${CERT_TMP}"; if [ -n "${ROUNDTRIP_TMP:-}" ]; then rm -f "${ROUNDTRIP_TMP}"; fi; }
trap cleanup EXIT
cert_pem > "${CERT_TMP}"
[ -s "${CERT_TMP}" ] || die "从 .p12 里取不出证书（文件损坏？）"

# sep_multiline 让每个字段单独一行，团队名里带逗号也不会把解析带偏
# （OpenSSL 3 与 macOS 自带的 LibreSSL 都支持；`=[[:space:]]*` 是为了容错两种排版）
subject="$(openssl x509 -in "${CERT_TMP}" -noout -subject -nameopt sep_multiline)"
issuer="$(openssl x509 -in "${CERT_TMP}" -noout -issuer -nameopt sep_multiline)"
# CN 与 OU 各取第一行（同名多值时 Apple 的证书不会出现，取第一个即可）
cert_cn="$(printf '%s\n' "${subject}" | sed -n 's/^[[:space:]]*CN[[:space:]]*=[[:space:]]*//p' | head -1)"
cert_ou="$(printf '%s\n' "${subject}" | sed -n 's/^[[:space:]]*OU[[:space:]]*=[[:space:]]*//p' | head -1)"

# ---------------------------------------------------------------------------
# 3. 证书类型：只有 Developer ID Application 能给分发的 .app 签名
# ---------------------------------------------------------------------------
case "${cert_cn}" in
  "${IDENTITY_PREFIX}"*) ok "证书类型是 Developer ID Application" ;;
  "Developer ID Installer:"*) die "这是 Developer ID Installer 证书（用于签 .pkg 安装包），不能给 .app 签名。
   请在 developer.apple.com 重新签发一张 Developer ID Application 证书。" ;;
  "Apple Development:"*|"Mac Developer:"*)
    die "这是开发用证书（${cert_cn}），不能给分发给用户的包签名，也过不了公证。
   请重新签发一张 Developer ID Application 证书。" ;;
  "") die "证书里读不出 CN（证书损坏，或者这个 openssl 不支持 -nameopt sep_multiline）：${subject}" ;;
  *) die "证书 CN 不是 Developer ID Application 身份：「${cert_cn}」
   签名与公证都要求 Developer ID Application 证书，请检查是不是拿错了证书。" ;;
esac

printf '%s\n' "${issuer}" | grep -q "Developer ID Certification Authority" \
  || warn "证书链的签发者不是 Developer ID Certification Authority（自签证书过不了公证，请确认这份证书是 Apple 签发的）"

# ---------------------------------------------------------------------------
# 4. 身份与团队 ID：以证书本身为准，不让人手抄
# ---------------------------------------------------------------------------
derived_identity="${cert_cn}"
if [ -n "${APPLE_SIGNING_IDENTITY:-}" ]; then
  [ "${APPLE_SIGNING_IDENTITY}" = "${derived_identity}" ] \
    || die "APPLE_SIGNING_IDENTITY 与证书里的身份不一致 —— CI 会用这个字符串挑签名身份，不一致就等于没签名：
   你填的 : ${APPLE_SIGNING_IDENTITY}
   证书里 : ${derived_identity}"
  ok "APPLE_SIGNING_IDENTITY 与证书一致"
else
  APPLE_SIGNING_IDENTITY="${derived_identity}"
  ok "签名身份取自证书：${APPLE_SIGNING_IDENTITY}"
fi

# Developer ID 证书的 OU 就是 10 位团队 ID；读不出来时才要求手工给
if printf '%s' "${cert_ou}" | grep -qE '^[A-Z0-9]{10}$'; then
  if [ -n "${APPLE_TEAM_ID:-}" ] && [ "${APPLE_TEAM_ID}" != "${cert_ou}" ]; then
    die "APPLE_TEAM_ID 与证书里的团队 ID 不一致 —— verify-macos-signing.sh 会比对产物里的 TeamIdentifier，不一致会让发版构建失败：
   你填的 : ${APPLE_TEAM_ID}
   证书里 : ${cert_ou}"
  fi
  APPLE_TEAM_ID="${cert_ou}"
  ok "团队 ID 取自证书：${APPLE_TEAM_ID}"
else
  [ -n "${APPLE_TEAM_ID:-}" ] || die "证书里读不出团队 ID（OU=${cert_ou:-空}），请显式给 APPLE_TEAM_ID（10 位，见 Apple Developer 账号页）"
  printf '%s' "${APPLE_TEAM_ID}" | grep -qE '^[A-Z0-9]{10}$' || die "APPLE_TEAM_ID 看起来不是 10 位团队 ID：${APPLE_TEAM_ID}"
  warn "证书里读不出团队 ID，改用你给的 ${APPLE_TEAM_ID}（请确认它与账号页一致）"
fi

# 过期证书签出来的包过不了 Gatekeeper，而这件事在本地最容易提前发现
enddate="$(openssl x509 -in "${CERT_TMP}" -noout -enddate | sed 's/^notAfter=//')"
if openssl x509 -in "${CERT_TMP}" -noout -checkend 0 >/dev/null 2>&1; then
  if openssl x509 -in "${CERT_TMP}" -noout -checkend 2592000 >/dev/null 2>&1; then
    ok "证书有效期到 ${enddate}"
  else
    warn "证书 ${enddate} 到期（30 天内）—— 到期后新签的包会被 Gatekeeper 拒绝，建议先在 Apple Developer 换一张"
  fi
else
  die "证书已于 ${enddate} 过期，签出来的包过不了 Gatekeeper。请在 Apple Developer 重新签发一张 Developer ID Application 证书。"
fi

# ---------------------------------------------------------------------------
# 5. 公证凭据：这两个值最容易「填错格子」
# ---------------------------------------------------------------------------
APPLE_ID="${APPLE_ID:-}"
[ -n "${APPLE_ID}" ] || die "缺少 APPLE_ID（公证用的 Apple 账号邮箱）"
printf '%s' "${APPLE_ID}" | grep -q "@" \
  || warn "APPLE_ID「${APPLE_ID}」看起来不是邮箱 —— 公证用的就是 Apple 账号的邮箱"
# 这两处刻意写成 if 而不是 `grep ... && die`：不依赖「errexit 是否豁免 AND 列表首个命令」这种细节
if printf '%s' "${APPLE_ID}" | grep -qE '[[:space:]]'; then
  die "APPLE_ID 里有空白字符：${APPLE_ID}"
fi

[ -n "${APPLE_PASSWORD}" ] || die "缺少 APPLE_PASSWORD（App 专用密码）"
if printf '%s' "${APPLE_PASSWORD}" | grep -q "@"; then
  die "APPLE_PASSWORD 看起来是邮箱（${APPLE_PASSWORD}）—— 这个位置要的是 App 专用密码，不是 Apple ID 本身"
fi
[ "${APPLE_PASSWORD}" != "${APPLE_ID}" ] || die "APPLE_PASSWORD 与 APPLE_ID 相同 —— 这里要的是 App 专用密码（appleid.apple.com 单独生成），不是登录密码"
app_pw_len="${#APPLE_PASSWORD}"
if printf '%s' "${APPLE_PASSWORD}" | grep -qE '^[a-z]{4}-[a-z]{4}-[a-z]{4}-[a-z]{4}$'; then
  ok "App 专用密码格式正确（${app_pw_len} 字符，不显示内容）"
elif [ "${ALLOW_PASSWORD_FORMAT:-0}" = "1" ]; then
  warn "APPLE_PASSWORD 不像 App 专用密码（形如 abcd-efgh-ijkl-mnop），但 ALLOW_PASSWORD_FORMAT=1 已放行"
else
  die "APPLE_PASSWORD 不像 App 专用密码（应为 4 组 4 位小写字母，形如 abcd-efgh-ijkl-mnop，长度 ${app_pw_len}）。
   用账号登录密码或 iCloud 密码都过不了公证（Apple 只接受 App 专用密码，在 appleid.apple.com → 登录与安全 生成）。
   如果你确认这就是对的格式，加 ALLOW_PASSWORD_FORMAT=1 放行。"
fi

# ---------------------------------------------------------------------------
# 6. 产出 APPLE_CERTIFICATE（base64）并做一次往返自检
# ---------------------------------------------------------------------------
# 换成单行 base64：BSD 的 `base64 -i` 是单行，GNU 的会按 76 列折行，
# 这里统一去掉空白，免得 CI 里 tauri 解不出证书（setup-macos-signing.sh 也有一道同样的兜底）
APPLE_CERTIFICATE="$(base64 < "${P12}" | tr -d '[:space:]')"
[ -n "${APPLE_CERTIFICATE}" ] || die "base64 编码失败（${P12} 读不出来？）"
APPLE_CERTIFICATE_PASSWORD="${P12_PASSWORD}"

b64_len="${#APPLE_CERTIFICATE}"
# GitHub 单个 secret 上限 48KB：超了会在 D 那一步才报错，这里先拦
[ "${b64_len}" -le 49152 ] || die "证书 base64 有 ${b64_len} 字节，超过 GitHub secret 的 48KB 上限（这份 .p12 里是不是夹带了证书链的多个证书？）"

# 往返自检：把 base64 解回去，确认它真能当 .p12 用、且密码对得上 ——
# CI 那边只拿到这串 base64，编码/截断出了问题在那里是「构建完才发现」
ROUNDTRIP_TMP="$(mktemp)"
printf '%s' "${APPLE_CERTIFICATE}" | base64 -d > "${ROUNDTRIP_TMP}" 2>/dev/null || die "证书 base64 解不回去（编码失败）"
P12_PASSWORD="${P12_PASSWORD}" openssl pkcs12 -in "${ROUNDTRIP_TMP}" -passin env:P12_PASSWORD -info -noout >/dev/null 2>&1 \
  || P12_PASSWORD="${P12_PASSWORD}" openssl pkcs12 -in "${ROUNDTRIP_TMP}" -passin env:P12_PASSWORD -info -noout -legacy >/dev/null 2>&1 \
  || die "base64 还原出来的 .p12 解不开（往返自检失败）—— 不要拿这份值去配 secret"
rm -f "${ROUNDTRIP_TMP}"; ROUNDTRIP_TMP=""
ok "APPLE_CERTIFICATE 往返自检通过（base64 还原后仍能解开，${b64_len} 字节）"

# 指纹用来回答「这份凭据对应哪张证书」：OpenSSL 3 打印 `sha256 Fingerprint=`，
# macOS 自带的 LibreSSL 打印 `SHA256 Fingerprint=`，所以这里不按固定大小写匹配
fingerprint="$(openssl x509 -in "${CERT_TMP}" -noout -fingerprint -sha256 | sed -E 's/^[A-Za-z0-9]+ Fingerprint=//' | tr -d ':')"

# ---------------------------------------------------------------------------
# 7. 写产出文件（D 的输入）：0600，别进仓库、别进聊天记录
# ---------------------------------------------------------------------------
if [ "${NO_WRITE:-0}" = "1" ]; then
  echo
  echo "• NO_WRITE=1：校验全部通过，没有写产出文件。6 个值都可用（下同，D 的输入由你自己保存）"
else
  out_dir="$(dirname "${OUT}")"
  mkdir -p "${out_dir}"
  ( umask 077; : > "${OUT}" )
  chmod 600 "${OUT}"
  # 注释里只放**非机密**的记账信息：身份、团队 ID、指纹、到期日 ——
  # 用来回答「这份文件对应哪张证书」，不泄露任何私钥 / 密码内容
  {
    printf '# macOS 代码签名 / 公证凭据（DEVELOPMENT.md §2.5 C / §8.10）—— 由 prepare-apple-signing.sh 生成\n'
    printf '# ⚠️ 含私钥（APPLE_CERTIFICATE 是带私钥的 .p12 的 base64）与密码，等同凭据本身：\n'
    printf '#    不要提交进仓库、不要贴进聊天记录 / 日志；离线副本请与私钥分开存放。\n'
    printf '# 证书身份 : %s\n' "${APPLE_SIGNING_IDENTITY}"
    printf '# 团队 ID  : %s\n' "${APPLE_TEAM_ID}"
    printf '# 证书指纹 : sha256 %s\n' "${fingerprint}"
    printf '# 证书到期 : %s\n' "${enddate}"
    printf '# 生成时间 : %s\n' "$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
    printf 'APPLE_CERTIFICATE=%s\n' "${APPLE_CERTIFICATE}"
    printf 'APPLE_CERTIFICATE_PASSWORD=%s\n' "${APPLE_CERTIFICATE_PASSWORD}"
    printf 'APPLE_SIGNING_IDENTITY=%s\n' "${APPLE_SIGNING_IDENTITY}"
    printf 'APPLE_ID=%s\n' "${APPLE_ID}"
    printf 'APPLE_PASSWORD=%s\n' "${APPLE_PASSWORD}"
    printf 'APPLE_TEAM_ID=%s\n' "${APPLE_TEAM_ID}"
  } > "${OUT}"
  ok "已写入 ${OUT}（权限 600，注释里只有身份 / 团队 ID / 指纹，没有密码）"
fi

# ---------------------------------------------------------------------------
# 汇总（只打印非机密信息：身份、团队 ID、指纹、长度 —— 不打印任何密码与 base64 内容）
# ---------------------------------------------------------------------------
cat <<EOF

• 凭据自检通过（6 个值都就位，且与证书本身对得上）：
    APPLE_SIGNING_IDENTITY       ${APPLE_SIGNING_IDENTITY}
    APPLE_TEAM_ID                ${APPLE_TEAM_ID}
    APPLE_ID                     ${APPLE_ID}
    APPLE_CERTIFICATE            base64 ${b64_len} 字节（含私钥，勿贴出）
    APPLE_CERTIFICATE_PASSWORD   ${#APPLE_CERTIFICATE_PASSWORD} 字符（不显示）
    APPLE_PASSWORD               ${app_pw_len} 字符（不显示）
  证书：sha256 ${fingerprint:0:16}…  到期 ${enddate}

• 下一步（§2.5 D）—— 把这 6 个值配进仓库，先演练再写入：
    bash scripts/configure-apple-secrets.sh                # 默认 DRY_RUN，逐个走一遍加密但不写入
    ALLOW_CONFIGURE=1 bash scripts/configure-apple-secrets.sh   # 确认无误后真正写入
EOF
