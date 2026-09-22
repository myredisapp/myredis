#!/usr/bin/env bash
#
# 核验「用户实际下载到的」macOS 产物能不能过 Gatekeeper（DEVELOPMENT.md §2.5 E）。
#
# 与 .github/scripts/verify-macos-signing.sh 的分工：
#   那份跑在**构建机上**，判据是 bundle 目录里的中间产物（还没改名的 .app 与 .dmg）；
#   这份跑在**任何一台 macOS 上**，判据是**从 Release 下载回来的那个 dmg** —— 也就是
#   用户双击的文件。两者的判据同源（codesign / spctl / stapler），但这份多做两件事：
#     ① 判之前先把 com.apple.quarantine 属性补上：浏览器下载的产物带这个属性，Gatekeeper
#        对带 quarantine 的产物才做完整评估（「首次打开被拦、需要右键打开」就是这个场景）。
#        本机自己构建的产物没有这个属性，直接判会宽松，所以这里显式补上。
#     ② 把 dmg 挂起来、把里面的 .app 拷出来再各判一遍：用户真正启动的是 dmg 里的那个 .app，
#        dmg 本身只过「能不能打开」那一关（spctl 的 disk image 形式）。
#
# 用法：
#   bash scripts/verify-macos-download.sh <dmg | .app | 目录>
#     EXPECT_TEAM_ID=ABCDE12345   断言 TeamIdentifier 与之一致
#                                 （不给就只要求「存在且不是 not set」）
#     EXPECT_VERSION=0.0.17       断言包内版本号（不给就跳过；能抓住「签的是上一版产物」）
#     SKIP_QUARANTINE=1           不打 quarantine 属性
#     KEEP_MOUNT=1                不自动卸载 / 不清理临时目录，便于排查
#   上面这些开关两种写法都认：写在命令前（`EXPECT_VERSION=... bash ...`，推荐）或跟在脚本名后
#   （`bash ... x.dmg EXPECT_VERSION=...`）—— 写反位置不该是「静默没生效」。
#
# 退出码：0 = 判据全过；1 = 有判据没过（逐条打印原因）；
#         2 = 用法 / 环境问题（含「本机 Gatekeeper 评估被关闭，spctl 这条验不到」）。
#
# ⚠️ 一条容易踩空的地方：`sudo spctl --master-disable` 之后（开发机常这么干，好放行下载来的 App），
# spctl 对**任何**产物都返回 accepted —— 连完全没签名的也照收（输出里带 override=security disabled）。
# 这时「能不能过 Gatekeeper」这条判据在本机是空的，绿灯是假的。所以脚本先看 `spctl --status`：
# 关闭时这条按「未验到」记（打印 ? 而不是 ✓），并以退出码 2 结束 —— 不会给出假的通过。
#
# 判据与 CI 里的 verify-macos-signing.sh 刻意保持一致，所以这里出现的失败原因也能直接
# 对照那边：codesign --verify（有没有签名）→ 身份必须是 Developer ID Application（ad-hoc
# 签名也能过 codesign，但对 Gatekeeper 毫无用处）→ TeamIdentifier 与配置一致 →
# spctl（Gatekeeper 本尊的判据，未公证会被拒）→ stapler（票据已随包 staple，离线首开也过）。
#
# 写法上只用 bash 3.2 就有的东西（macOS 自带的是 3.2，没有 declare -A）—— 与 scripts/ 下其它脚本一致。
set -euo pipefail

die() { echo "✗ $*" >&2; exit 1; }
env_die() { echo "✗ $*" >&2; exit 2; }
ok() { echo "  ✓ $*"; }
bad() { echo "  ✗ $*"; }

TARGET=""
# 开关写成 `NAME=值` 的环境变量前缀（推荐），也认「跟在脚本名后面」的写法
# （`bash scripts/verify-macos-download.sh x.dmg EXPECT_TEAM_ID=...`）—— 与 scripts/ 下
# configure-apple-secrets.sh / verify-prerelease-release.sh 同款：写反位置不该是「静默没生效」
for arg in "$@"; do
  case "${arg}" in
    EXPECT_TEAM_ID=*|EXPECT_VERSION=*|SKIP_QUARANTINE=*|KEEP_MOUNT=*) export "${arg}" ;;
    *)
      [ -z "${TARGET}" ] || env_die "只接受一个待判产物，多余的参数：${arg}（用法见脚本头部注释）"
      TARGET="${arg}"
      ;;
  esac
done

[ -n "${TARGET}" ] || env_die "用法：verify-macos-download.sh <dmg | .app | 目录>（细节见脚本头部注释）"
[ -e "${TARGET}" ] || env_die "找不到 ${TARGET}"

[ "$(uname -s)" = "Darwin" ] || env_die "这个判据只有 macOS 上有：codesign / spctl / stapler 依赖系统工具"

for tool in codesign spctl xcrun; do
  command -v "${tool}" >/dev/null 2>&1 || env_die "找不到 ${tool}（需要 Xcode Command Line Tools：xcode-select --install）"
done

# ---------------------------------------------------------------------------
# 0. 定位要判的产物：给目录时优先 dmg（用户下载的就是它），其次 .app
# ---------------------------------------------------------------------------
DMG=""
APP=""
case "${TARGET}" in
  *.dmg) DMG="${TARGET}" ;;
  *.app) APP="${TARGET}" ;;
  *)
    if [ -d "${TARGET}" ]; then
      DMG="$(find "${TARGET}" -maxdepth 3 -name '*.dmg' -print -quit 2>/dev/null || true)"
      [ -n "${DMG}" ] || APP="$(find "${TARGET}" -maxdepth 3 -name '*.app' -print -quit 2>/dev/null || true)"
    fi
    ;;
esac
[ -n "${DMG}" ] || [ -n "${APP}" ] || env_die "${TARGET} 里既没有 .dmg 也没有 .app（给的是下载回来的安装包吗？）"

STAGE="$(mktemp -d "${TMPDIR:-/tmp}/myredis-verify-download.XXXXXX")"

# spctl 只在**本机 Gatekeeper 评估开着**的时候才是个判据：`sudo spctl --master-disable`
# （开发机常用来放行下载的 App）之后它对任何产物都返回 accepted，包括完全没签名的 ——
# 那样这条判据就成了「空判据」，绿灯是假的。所以先看状态，关闭时这条判据按「未验到」处理。
SPCTL_USABLE=1
SPCTL_STATUS="$(spctl --status 2>&1 || true)"
case "${SPCTL_STATUS}" in
  *enabled*) ;;
  *)
    SPCTL_USABLE=0
    echo "! 本机 Gatekeeper 评估是关闭的（spctl --status：${SPCTL_STATUS:-读不出}）——"
    echo "  spctl 现在对任何产物都返回 accepted（输出里会带 override=security disabled），"
    echo "  所以「能不能过 Gatekeeper」这条在本机**验不到**。要在本机看这条：sudo spctl --master-enable"
    ;;
esac

FAILED=0
PASSED=0
INCONCLUSIVE=0
# spctl 判据包一层：本机评估被关掉时按「未验到」记，不能算过（见上面的说明）
judge_spctl() {
  local label="$1"
  shift
  if [ "${SPCTL_USABLE}" -eq 0 ]; then
    echo "  ? ${label} —— 未验到（本机 Gatekeeper 评估已关闭）"
    INCONCLUSIVE=$((INCONCLUSIVE + 1))
    return 0
  fi
  judge "${label}" "$@"
}

# 每条判据都跑完再汇总：只报第一个失败会让人来回试，而这里每条都是独立的判据
judge() {
  local label="$1" log="${STAGE}/cmd.log"
  shift
  if "$@" > "${log}" 2>&1; then
    ok "${label}"
    PASSED=$((PASSED + 1))
    return 0
  fi
  bad "${label}"
  sed 's/^/      /' "${log}" | tail -n 20
  FAILED=$((FAILED + 1))
  return 1
}

# ---------------------------------------------------------------------------
# 1. 补 quarantine 属性：让判据等价于「用户刚下载完双击」
# ---------------------------------------------------------------------------
# 格式是 `flag;十六进制时间戳;下载者;UUID`，具体取值不影响 Gatekeeper 的评估结论 ——
# 关键是**有这个属性**（它把「从网络下载来的」这件事告诉 Gatekeeper）。
quarantine() {
  [ "${SKIP_QUARANTINE:-0}" = "1" ] && return 0
  local stamp uuid
  stamp="$(printf '%x' "$(date +%s)")"
  uuid="$(uuidgen 2>/dev/null || echo 0)"
  xattr -w com.apple.quarantine "0081;${stamp};myredis-download-check;${uuid}" "$1" 2>/dev/null \
    || echo "  ! 打 quarantine 属性失败（${1}）：判据会宽松一点，但下面的结论仍然有效" >&2
}

MOUNTED=""
cleanup() {
  if [ "${KEEP_MOUNT:-0}" = "1" ]; then
    echo "• KEEP_MOUNT=1：保留挂载点 ${MOUNTED:-（未挂载）} 与临时目录 ${STAGE}"
    return 0
  fi
  if [ -n "${MOUNTED}" ]; then
    hdiutil detach "${MOUNTED}" -quiet >/dev/null 2>&1 || true
  fi
  rm -rf "${STAGE}" 2>/dev/null || true
  return 0
}
trap cleanup EXIT

if [ -n "${DMG}" ]; then
  echo "• 用户下载的安装包：${DMG}"
  echo "  大小 $(du -h "${DMG}" | cut -f1)（sha256 $(shasum -a 256 "${DMG}" | cut -c1-16)…）"
  quarantine "${DMG}"

  # dmg 自己的签名 / 票据：dmg 被 staple 与否取决于 tauri 版本（CI 里同样只告警），
  # 但「打开 dmg 会不会被 Gatekeeper 拦」是硬判据 —— 用 spctl 的 disk image 形式，与 Finder 一致
  judge "dmg 签名有效（codesign --verify）" codesign --verify --verbose=2 "${DMG}" || true
  judge_spctl "dmg 通过 Gatekeeper（spctl -t open，Finder 双击的判据）" \
    spctl -a -t open --context context:primary-signature -v "${DMG}" || true
  if xcrun stapler validate "${DMG}" >/dev/null 2>&1; then
    ok "dmg 已 staple 公证票据"
    PASSED=$((PASSED + 1))
  else
    echo "  ! dmg 上没有 staple 的公证票据（不拦：里面的 .app 另有硬判据，见下）"
  fi

  # 挂载后把 .app 拷出来：用户在 Finder 里就是把 .app 拖到「应用程序」的，
  # ditto 与 Finder 的拷贝语义一致（保留符号链接 / 资源分支），而 cp -R 不保证
  MOUNTED="${STAGE}/mnt"
  mkdir -p "${MOUNTED}"
  echo "• 挂载 dmg（只读）"
  hdiutil attach "${DMG}" -nobrowse -readonly -mountpoint "${MOUNTED}" >/dev/null \
    || die "挂载失败：${DMG}（文件损坏？下载不完整？）"

  APP="$(find "${MOUNTED}" -maxdepth 2 -name '*.app' -print -quit 2>/dev/null || true)"
  [ -n "${APP}" ] || die "dmg 里没有 .app —— 安装包结构不对，用户装出来的是个空盘"
  APP_COPY="${STAGE}/$(basename "${APP}")"
  ditto "${APP}" "${APP_COPY}" || die "把 .app 从 dmg 拷出来失败"
  echo "• dmg 内的 .app：$(basename "${APP}")（已拷到临时目录，按「拖进应用程序」处理）"
  APP="${APP_COPY}"
else
  echo "• 待判产物：${APP}（未经过 dmg 包装）"
fi

# ---------------------------------------------------------------------------
# 2. .app 的硬判据
# ---------------------------------------------------------------------------
quarantine "${APP}"

judge ".app 签名有效且完整（codesign --verify --deep --strict）" \
  codesign --verify --deep --strict --verbose=2 "${APP}" || true

# codesign -dv 的输出在 stderr；它同时也是「身份是什么」的唯一来源
SIGN_INFO="${STAGE}/codesign.txt"
codesign -dv --verbose=4 "${APP}" > /dev/null 2>"${SIGN_INFO}" || true
AUTHORITY="$(grep -m1 '^Authority=' "${SIGN_INFO}" | cut -d= -f2- || true)"
TEAM_ID="$(grep -m1 '^TeamIdentifier=' "${SIGN_INFO}" | cut -d= -f2- || true)"
FLAGS="$(grep -m1 '^flags=' "${SIGN_INFO}" | cut -d= -f2- || true)"
echo "    身份：${AUTHORITY:-（读不出）}"
echo "    团队：${TEAM_ID:-（读不出）}"
echo "    标志：${FLAGS:-（读不出）}"

judge "签名身份是 Developer ID Application（ad-hoc 签名对 Gatekeeper 无用）" \
  grep -q '^Authority=Developer ID Application' "${SIGN_INFO}" || true

if [ -n "${EXPECT_TEAM_ID:-}" ]; then
  judge "TeamIdentifier 与配置一致（${EXPECT_TEAM_ID}）" \
    grep -qx "TeamIdentifier=${EXPECT_TEAM_ID}" "${SIGN_INFO}" || true
else
  if [ -n "${TEAM_ID}" ] && [ "${TEAM_ID}" != "not set" ]; then
    ok "TeamIdentifier 存在：${TEAM_ID}（未给 EXPECT_TEAM_ID，未做一致性断言）"
    PASSED=$((PASSED + 1))
  else
    bad "产物里没有 TeamIdentifier —— 不是 Developer ID 证书签的"
    FAILED=$((FAILED + 1))
  fi
fi

# Gatekeeper 本尊：签名有效但**没公证**时它会拒绝，正是「自己机器能装、用户装不上」的那种
judge_spctl "通过 Gatekeeper 评估（spctl，未公证会被拒）" \
  spctl -a -vvv -t exec "${APP}" || true

judge "公证票据已 staple（离线首次打开也靠它）" \
  xcrun stapler validate "${APP}" || true

if [ -n "${EXPECT_VERSION:-}" ]; then
  PLIST="${APP}/Contents/Info.plist"
  ACTUAL_VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "${PLIST}" 2>/dev/null || true)"
  if [ "${ACTUAL_VERSION}" = "${EXPECT_VERSION}" ]; then
    ok "包内版本号一致：${ACTUAL_VERSION}"
    PASSED=$((PASSED + 1))
  else
    bad "包内版本号是「${ACTUAL_VERSION}」，期望「${EXPECT_VERSION}」——签的是不是上一版产物？"
    FAILED=$((FAILED + 1))
  fi
fi

echo
if [ "${FAILED}" -eq 0 ] && [ "${INCONCLUSIVE}" -eq 0 ]; then
  echo "✓ ${PASSED} 条判据全过：用户下载到的这份产物，双击首装不会被 Gatekeeper 拦"
  echo "  （仍建议在一台没装过本应用的 macOS 上人工双击确认一次，见 §2.5 E 的收尾）"
  exit 0
fi

if [ "${FAILED}" -eq 0 ]; then
  echo "? ${PASSED} 条判据通过，但有 ${INCONCLUSIVE} 条在本机**验不到**（Gatekeeper 评估被关闭）" >&2
  echo "  产物本身没发现问题，但「能不能过 Gatekeeper」这句话还不能下结论。" >&2
  echo "  办法：sudo spctl --master-enable 后重跑，或在一台 Gatekeeper 正常的 macOS 上跑（§2.5 E）。" >&2
  exit 2
fi

echo "✗ ${FAILED} 条判据没过（通过 ${PASSED} 条，另 ${INCONCLUSIVE} 条未验到）——逐条原因见上" >&2
cat >&2 <<'EOF'
  常见对应关系（与 §8.10 的表格一致）：
    · 没有签名 / 「code object is not signed at all」→ CI 走了「未配凭据」告警分支，或凭据没生效（§2.5 C / D）
    · 身份不是 Developer ID Application           → 拿成 Apple Development / Mac Developer 证书了
    · TeamIdentifier 不一致                       → APPLE_TEAM_ID 与证书不是一套
    · spctl 被拒（code=0 之外）                   → 多半没公证：APPLE_PASSWORD 是不是用了登录密码？
    · stapler validate 失败                       → 公证没做完就被打包，或票据没 staple
EOF
exit 1
