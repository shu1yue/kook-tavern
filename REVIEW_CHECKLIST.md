# 代码审查硬性检查清单（REVIEW_CHECKLIST）

每个里程碑（M0.5 / M1 / M2 / M3 / M4）合并至 `main` 或发布至云端前，必须由独立上下文审查员对照本清单逐项核查：

## 1. 架构解耦与防屎山红线
- [ ] 分层依赖严格单向：`domain` 零 IO / 零网络 / 零数据库；`core` 仅依赖 `domain` 与抽象端口 Trait；各 `adapter-*` 严禁互相依赖。
- [ ] 单文件职责单一，生产代码文件原则上控制在 **350 行以内**（纯测试代码可单独拆分）。
- [ ] 主二进制产物路径保持 `target/release/kook-tavern-dm`，启动子命令保持 `kook` / `local`，不破坏云端 `systemd` 单元兼容性。

## 2. 并发安全与状态一致性
- [ ] **严禁跨 `.await` 长时间持有房间锁或 SQLite 互斥锁**。
- [ ] 回合结算严格走显式状态机（`Collecting` → `Resolving` → `Collecting`），且落库使用 `state_version` 乐观锁 CAS 校验，防止 `/undo` 或 `/reset` 后的慢响应写入脏数据。
- [ ] LLM 调用超时、网络报错或解析失败时，房间状态必须确定性回退到 `Collecting`，绝不永久卡死。
- [ ] `Resolving` 期间新提交的 `/act` / `/roll` 必须按 `target_turn` 隔离在下一轮缓冲区，绝不被本轮结算清理误删。

## 3. 数据迁移与边界容错
- [ ] SQLite 结构变更必须通过递增版本号迁移脚本（`migrations/`）实现，旧数据库升级幂等且零数据丢失。
- [ ] AI 返回的 `hp_delta`、`san_delta`、道具增删与线索全部经后端权威范围校验后再落库；非 JSON 输出可平滑降级，绝不 panic。
- [ ] 外部 HTTP / WebSocket 均配置合理超时与断线重连退避。

## 4. 安全保密与同机共存红线
- [ ] 任何日志、`tracing` 字段、`Debug` 输出与错误文案均**严禁回显** `KOOK_BOT_TOKEN`、`LLM_API_KEY` 或真实服务器凭据。
- [ ] `.gitignore` 严格屏蔽 `.env`、`DEPLOY_ECS.md`、`data/`、`exports/`、`target/`，Git 暂存区无敏感文件或公网 IP 泄露。
- [ ] 严禁占用或触碰同机保留端口（`8080` / `3000` / `6099`）及其他服务路径；新增依赖精简，保障 2C2G 单核构建与 `MemoryMax=180M` 运行余量。
