# ECS 部署指南模板（脱敏公开版）

> 本文件仅保留标准部署流程，真实服务器 IP、SSH 凭据路径与环境密钥请保存在本地且已加入 `.gitignore` 的 `DEPLOY_ECS.md` 与 `.env` 中，严禁提交到 Git 仓库。

## 1. 服务器前置要求
- Ubuntu 24.04（或兼容 Linux 发行版），已安装 `rustc >= 1.85` / `cargo`、`sqlite3`、`rsync`。
- 建议使用独立低权限系统用户（如 `tavern:tavern`）配合 `systemd` 托管单二进制 `/usr/local/bin/kook-tavern-dm kook`。
- 2C2G 等小内存机器编译时务必指定 `CARGO_BUILD_JOBS=1`，防止内存峰值影响同机服务。

## 2. 标准更新流程
1. 同步源码（排除运行态与敏感文件）：
   ```bash
   rsync -az --delete \
     --exclude target --exclude .env --exclude .git --exclude data --exclude exports --exclude DEPLOY_ECS.md \
     ./ root@<YOUR_ECS_IP>:/opt/kook-tavern-dm/
   ```
2. 备份现有数据库与上一版可执行文件（供 10 秒快速回滚）：
   ```bash
   sqlite3 /opt/kook-tavern-dm/data/tavern.db ".backup '/opt/kook-tavern-backups/tavern_$(date +%Y%m%d_%H%M%S).db'"
   cp -f /usr/local/bin/kook-tavern-dm /usr/local/bin/kook-tavern-dm.prev 2>/dev/null || true
   ```
3. 单核增量编译并原子替换重启：
   ```bash
   cd /opt/kook-tavern-dm
   CARGO_BUILD_JOBS=1 cargo build --release -p kook-tavern-dm
   install -m 755 target/release/kook-tavern-dm /usr/local/bin/kook-tavern-dm.new
   mv -f /usr/local/bin/kook-tavern-dm.new /usr/local/bin/kook-tavern-dm
   systemctl restart kook-tavern-dm.service
   systemctl is-active kook-tavern-dm.service
   ```
