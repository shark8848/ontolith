# Ontolith Server 后台 / systemd 部署

文档 ID: OPS-L5-0001  
版本: 1.1.0  
日期: 2026-07-17

---

## 当前状态（本机）

| 项 | 值 |
|----|-----|
| 模式 | **user systemd**（无需 root） |
| unit | `~/.config/systemd/user/ontolith-server.service` |
| 配置 | `~/.config/ontolith/ontolith.env` |
| 二进制 | `/home/ontolith/target/release/ontolith-server` |
| 监听 | `http://127.0.0.1:8090`（8080/8081 已被占用） |
| 开机自启 | `enabled`（用户会话内；logout 后需 linger） |

验证：

```bash
systemctl --user status ontolith-server
curl -s http://127.0.0.1:8090/health
curl -s http://127.0.0.1:8090/cluster/status
```

---

## 一、用户服务（推荐，无 sudo）

### 安装 / 更新

```bash
cargo build -p ontolith-server --release
./scripts/install-ontolith-user-service.sh

# 管理服务（统一管理面）
cargo build -p ontolith-server --release --bin ontolith-management-server
./scripts/install-ontolith-management-user-service.sh
```

### 常用命令

```bash
systemctl --user status ontolith-server
systemctl --user restart ontolith-server
systemctl --user stop ontolith-server
systemctl --user disable ontolith-server   # 取消自启
journalctl --user -u ontolith-server -f   # 日志

systemctl --user status ontolith-management-server
systemctl --user restart ontolith-management-server
journalctl --user -u ontolith-management-server -f
```

### 改端口 / 存储 / 鉴权

编辑 `~/.config/ontolith/ontolith.env`：

```bash
ONTOLITH_BIND=127.0.0.1:8090
ONTOLITH_STORAGE=memory          # 或 rocksdb
ONTOLITH_DATA_DIR=/home/ontolith/data
ONTOLITH_AUTH_MODE=disabled      # 或 enforced
# ONTOLITH_API_KEY=change-me
```

然后：

```bash
systemctl --user restart ontolith-server
```

### 注销后仍保持运行（可选，需 root 一次）

```bash
sudo loginctl enable-linger "$USER"
```

---

## 二、系统服务（需 sudo）

适用于开机即启、多用户机器：

```bash
cargo build -p ontolith-server --release
./scripts/install-ontolith-system-service.sh

# 管理服务（统一管理面）
cargo build -p ontolith-server --release --bin ontolith-management-server
./scripts/install-ontolith-management-system-service.sh
```

- unit: `/etc/systemd/system/ontolith-server.service`
- env: `/etc/ontolith/ontolith.env`
- binary: `/usr/local/bin/ontolith-server`
- 默认 bind: `127.0.0.1:8080`

```bash
sudo systemctl status ontolith-server
sudo journalctl -u ontolith-server -f

sudo systemctl status ontolith-management-server
sudo journalctl -u ontolith-management-server -f
```

> 当前环境无交互 sudo 密码时，请在本机终端自行执行上述脚本。

---

## 三、文件清单

| 路径 | 用途 |
|------|------|
| `deployments/systemd-user/ontolith-server.service` | user unit 模板 |
| `deployments/systemd-user/ontolith-management-server.service` | management user unit 模板 |
| `deployments/ontolith-server.service` | system unit 模板 |
| `deployments/ontolith-management-server.service` | management system unit 模板 |
| `deployments/ontolith.user.env` | user 环境模板 |
| `deployments/ontolith-management.user.env` | management user 环境模板 |
| `deployments/ontolith.env` | system 环境模板 |
| `deployments/ontolith-management.env` | management system 环境模板 |
| `scripts/install-ontolith-user-service.sh` | 安装 user 服务 |
| `scripts/install-ontolith-system-service.sh` | 安装 system 服务 |
| `scripts/install-ontolith-management-user-service.sh` | 安装 management user 服务 |
| `scripts/install-ontolith-management-system-service.sh` | 安装 management system 服务 |

---

## 四、排障

| 现象 | 处理 |
|------|------|
| `Address already in use` | 改 `ONTOLITH_BIND` 或结束占用进程 |
| 服务反复 restart | `journalctl --user -u ontolith-server -n 50` |
| 更新代码后仍旧行为 | 先 `cargo build -p ontolith-server --release` 再 `systemctl --user restart` |
| 登出后服务停 | `sudo loginctl enable-linger $USER` 或改 system unit |

---

## 五、管理面健康检查

```bash
curl -s http://127.0.0.1:9091/admin/health
curl -s http://127.0.0.1:9091/admin/config
curl -s http://127.0.0.1:9091/admin/monitoring
```

## 六、管理面窗口化 SLO 检查

可使用短窗口脚本验证 `runtime_probe` 成功率与延迟阈值：

```bash
ONTOLITH_MANAGEMENT_MONITORING_URL=http://127.0.0.1:9091/admin/monitoring \
ONTOLITH_MANAGEMENT_SLO_WINDOW_SAMPLES=12 \
ONTOLITH_MANAGEMENT_SLO_WINDOW_INTERVAL_SEC=5 \
ONTOLITH_MANAGEMENT_SLO_MIN_SUCCESS_PERCENT=99 \
ONTOLITH_MANAGEMENT_SLO_P95_MAX_LATENCY_MS=250 \
bash scripts/check-management-slo-window.sh
```

如果服务以 systemd 运行，也可将阈值写入 `ontolith-management.env` / `ontolith-management.user.env`，用于统一运维基线。
