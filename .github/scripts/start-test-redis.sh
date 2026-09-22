#!/usr/bin/env bash
# 拉起测试用 Redis 环境：单机（6379）+ TLS 单机（6390，自签名证书）+
# 三主节点集群（7001-7003，无副本）。
# 这是 src-tauri/tests 里 #[ignore] 集成用例的前置条件（见 DEVELOPMENT.md §2.3 / §7）。
# 已在 CI（ubuntu-22.04）与本地（macOS + homebrew redis）验证。
set -euo pipefail

# GitHub ubuntu runner 预装了 redis-server，但不做这个假设：缺了就装
if ! command -v redis-server >/dev/null 2>&1; then
  sudo apt-get update
  sudo apt-get install -y redis-server
fi

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

echo "单机 $(redis-cli -p 6379 ping)，集群入口 $(redis-cli -p 7001 cluster info | grep ^cluster_state)"
