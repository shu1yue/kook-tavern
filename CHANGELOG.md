# 变更日志与快速回滚手册（CHANGELOG）

> 每次版本上线前，用大白话记录“这次改了什么”以及“万一线上出状况怎么 10 秒回滚”。

## [Unreleased] - M0.5 工程化底座重构
### 本次改动
- 将单体代码拆分为 7 个职责独立的 Rust Workspace Crate（`domain` / `core` / `infra-sqlite` / `infra-llm` / `adapter-kook` / `adapter-cli` / `app`），彻底隔离业务规则、存储、大模型与聊天平台。
- 引入显式回合状态机（`Collecting` / `Resolving`）与 SQLite `state_version` 乐观锁，修复结算期间提交新行动可能被清空、慢响应覆盖 `/undo` 脏写的隐患。
- 新增 SQLite 递增版本号增量迁移（`0001_init.sql` 与 `0002_m05_state_machine`），兼容旧版 `data/tavern.db` 及历史快照无损升级与 `/undo`。
- 将 DM 系统提示词与叙事风格抽离至外部文件 `prompts/*.md`，房主不懂代码也可直接像记事本一样微调 DM 文风并即时生效。
- 补齐 `AppConfig` 密钥脱敏、`.gitignore` 敏感运维文档屏蔽、`REVIEW_CHECKLIST.md` 与 GitHub Actions CI 质检流水线。

### 线上紧急 10 秒回滚命令（云端已备份旧二进制时）
```bash
mv -f /usr/local/bin/kook-tavern-dm.prev /usr/local/bin/kook-tavern-dm
systemctl restart kook-tavern-dm.service
systemctl is-active kook-tavern-dm.service
```

---

## [0.1.0] - M0 初始原型
- 完成 KOOK API v3 WebSocket/卡片消息基础接入、本地终端模拟器、确定性掷骰、世界书关键词触发、SQLite 快照 `/undo` 与双内置剧本（《余烬酒馆》《黑杉庄园》）。
