#!/usr/bin/env bash
# 拉起测试用 Redis 环境：单机（6379）+ TLS 单机（6390，自签名证书）+
# 三主节点集群（7001-7003，无副本）。
# 这是 src-tauri/tests 里 #[ignore] 集成用例的前置条件（见 DEVELOPMENT.md §2.3 / §7）。
# 已在 CI（ubuntu-22.04 / 24.04）与本地（macOS + homebrew redis）验证。
#
# 环境变量：
#   REDIS_VERSION_EXPECT  版本门（版本前缀，如 `6.0` / `7.`）。CI 的版本矩阵用它把
#                         「这一轮跑的是哪个 Redis」钉死：runner 镜像换代换了 Redis 版本时
#                         测试直接失败，而不是悄悄在别的版本上跑（见 DEVELOPMENT.md §2.4）。
set -euo pipefail

# GitHub ubuntu runner 预装了 redis-server，但不做这个假设：缺了就装
if ! command -v redis-server >/dev/null 2>&1; then
  sudo apt-get update
  sudo apt-get install -y redis-server
fi

# 端口上已有 Redis 在跑时：CI 里直接关掉（runner 是一次性环境，多半是镜像预装的服务，
# 或上一步 apt 装完后自启的服务 —— 不清掉下面的 daemonize 会因端口占用失败），
# 本地则停下来让人自己处理：绝不静默关掉开发者本机正在用的实例。
ensure_port_free() {
  local port=$1
  redis-cli -p "$port" ping >/dev/null 2>&1 || return 0
  if [ -n "${CI:-}" ]; then
    echo "• $port 上已有 Redis（镜像预装 / apt 装完自启），先关掉它再拉起测试实例"
    redis-cli -p "$port" shutdown nosave >/dev/null 2>&1 || true
    return 0
  fi
  echo "✗ $port 上已有 Redis 在跑。本脚本要自己拉起测试实例，请先停掉它" >&2
  echo "  （brew services stop redis，或 redis-cli -p $port shutdown）" >&2
  exit 1
}

# 版本门（可选）：以 redis-server 自己的版本输出为准
redis_server_version() {
  redis-server --version | sed -n 's/.*v=\([0-9][0-9.]*\).*/\1/p'
}

if [ -n "${REDIS_VERSION_EXPECT:-}" ]; then
  actual_version="$(redis_server_version)"
  case "$actual_version" in
    "${REDIS_VERSION_EXPECT}"*)
      echo "• 版本门通过：redis-server ${actual_version}（期望前缀 ${REDIS_VERSION_EXPECT}）"
      ;;
    *)
      echo "::error::期望 Redis ${REDIS_VERSION_EXPECT}.x，实际是 ${actual_version:-未知版本}。runner 镜像或 apt 源换了版本，请确认测试矩阵是否要跟着调整" >&2
      exit 1
      ;;
  esac
fi

ensure_port_free 6379

# 单机：测试连接写死 127.0.0.1:6379
redis-server --port 6379 --daemonize yes --save '' --appendonly no

# 集群：测试用 MYREDIS_CLUSTER_PORT 默认 7001 作为入口节点，
# 只需能发现整个集群，起 3 个主节点即可（无副本，省资源）
start_cluster_node() {
  local port=$1
  redis-server --port "$port" \
    --daemonize yes \
    --save '' --appendonly no \
    --cluster-enabled yes \
    --cluster-config-file "nodes-${port}.conf" \
    --cluster-node-timeout 3000 \
    --dir "$(mktemp -d)"
}

for port in 7001 7002 7003; do
  ensure_port_free "$port"
done

start_cluster_node 7001
start_cluster_node 7002
start_cluster_node 7003

for port in 7001 7002 7003; do
  for _ in $(seq 1 30); do
    if redis-cli -p "$port" ping 2>/dev/null | grep -q PONG; then
      break
    fi
    sleep 1
  done
  redis-cli -p "$port" ping | grep -q PONG || { echo "::error::${port} 节点未就绪"; exit 1; }
done

redis-cli --cluster create \
  127.0.0.1:7001 127.0.0.1:7002 127.0.0.1:7003 \
  --cluster-replicas 0 --cluster-yes

# TLS 单机：ping_integration.rs 里 tls_connect_* 用例连 127.0.0.1:6390，
# 需自签名证书（校验模式用例靠「CA 不被信任」触发证书错误）
CERT_DIR="$(mktemp -d)"
openssl req -x509 -newkey rsa:2048 -nodes -days 1 \
  -keyout "$CERT_DIR/ca.key" -out "$CERT_DIR/ca.crt" \
  -subj "/CN=myredis-test-ca" >/dev/null 2>&1
openssl req -new -newkey rsa:2048 -nodes \
  -keyout "$CERT_DIR/server.key" -out "$CERT_DIR/server.csr" \
  -subj "/CN=127.0.0.1" >/dev/null 2>&1
printf 'subjectAltName=IP:127.0.0.1\n' > "$CERT_DIR/server.ext"
openssl x509 -req -in "$CERT_DIR/server.csr" \
  -CA "$CERT_DIR/ca.crt" -CAkey "$CERT_DIR/ca.key" -CAcreateserial \
  -out "$CERT_DIR/server.crt" -days 1 \
  -extfile "$CERT_DIR/server.ext" >/dev/null 2>&1

# --port 0 关闭非 TLS 端口，只留 tls-port；客户端校验模式因 CA 不可信必失败，符合用例预期。
# tls-auth-clients no：客户端（redis-cli 与 rustls 连接池）都不带客户端证书，
# 而 Redis 7+ 默认要求客户端证书，不显式关掉 TLS 握手会被拒
redis-server --port 0 --tls-port 6390 --daemonize yes \
  --save '' --appendonly no \
  --tls-cert-file "$CERT_DIR/server.crt" \
  --tls-key-file "$CERT_DIR/server.key" \
  --tls-ca-cert-file "$CERT_DIR/ca.crt" \
  --tls-auth-clients no \
  --dir "$CERT_DIR"

# 用自建 CA 校验（而非 --insecure）：ubuntu-22.04 的 redis-cli 6.0 不认识 --insecure
for _ in $(seq 1 30); do
  if redis-cli --tls --cacert "$CERT_DIR/ca.crt" -p 6390 ping 2>/dev/null | grep -q PONG; then
    break
  fi
  sleep 1
done
redis-cli --tls --cacert "$CERT_DIR/ca.crt" -p 6390 ping | grep -q PONG \
  || { echo "::error::TLS 节点（6390）未就绪"; exit 1; }

echo "单机 $(redis-cli -p 6379 ping)，TLS $(redis-cli --tls --cacert "$CERT_DIR/ca.crt" -p 6390 ping)，集群入口 $(redis-cli -p 7001 cluster info | grep ^cluster_state)"

# create 返回不代表槽已分配完成，等集群自检进入 ok 态
for _ in $(seq 1 30); do
  if redis-cli -p 7001 cluster info 2>/dev/null | grep -q '^cluster_state:ok'; then
    break
  fi
  sleep 1
done
redis-cli -p 7001 cluster info | grep -q '^cluster_state:ok' \
  || { echo "::error::集群未进入 ok 态"; exit 1; }

# 把实际版本打进日志：排查「只有某个版本红」的问题时，第一眼就要看到它
echo "测试环境就绪 —— $(redis-server --version)，$(redis-cli --version)"
echo "单机 $(redis-cli -p 6379 ping)，集群入口 $(redis-cli -p 7001 cluster info | grep ^cluster_state)"
