# 变更日志与快速回滚手册（CHANGELOG）

> 每次版本上线前，用大白话记录“这次改了什么”以及“万一线上出状况怎么 10 秒回滚”。

## [0.4.0] - Wave 1 剧本大扩充 + 职业一键选卡 + NPC 记忆 + 战报墙
### 本次改动
- **三大深度剧本库**：《余烬酒馆与迷雾委托》《雨夜黑杉庄园》双本扩充至各 **10 条世界书 + 4 个特色预设职业 + 3 种结局收束**；全新推出第 3 个中式都市规则怪谈剧本 **《午夜四号线 · 末班车怪谈》**（`midnight_metro.yaml`）。
- **预设职业一键选卡**：新增 `RolePicker` 卡片与 `/roles` / `🎭 选职业入团` 按钮，新人点一下即携带职业专属装备与特质上桌；老玩家换职业自动保留原 HP/SAN/背包，严防重复刷道具。
- **活着的 NPC 态度记忆栏**：新增 SQLite 增量迁移 `0006_m1_npc_states` 与 DM JSON `npc_updates`，持久追踪关键 NPC 的态度（警惕/信任/敌对等）与动向，并在战局面板、Prompt 上下文和 `/undo` 回档中全程同步。
- **一键高光战报直发 `📜冒险战报墙`**：新增 `EXPORT_CHANNEL_ID` 配置与 `📜 发战报墙` 按钮，导出 Markdown 同时把高光战报卡自动张贴至战报墙频道（绝不泄露任何私密纸条）。

## [0.3.1] - M1.1 KOOK 卡片 UI 焕新与零代码全按钮交互
### 本次改动
- **零代码全按钮操控**：新增 `crates/core/src/commands/buttons.rs`，支持一键选本开团（`start` / `menu`）、一键快速入团（`quickjoin`，自动避重名）、直接点击 DM 剧情灵感按钮提交行动（`pick`，绑定回合号防误点旧卡）、一键掷骰（`2d6` / `1d100`）、一键查秘密纸条（`whisper`，仅本人可见）、一键暂离/归队（`afk`）、一键切换语音/文字档与直接说话/指令前缀模式（`toggle_style` / `toggle_mode`）、一键查账单（`cost`）。
- **开团体验优化**：`/start` 默认开启 `Roleplay`（直接打人话即行动，括号/双斜杠跳过场外闲聊），内置双剧本新增开场可点击行动灵感 `opening_actions`，新增 SQLite 增量迁移 `0005_m1_ux_suggested_actions`。
- **卡片颜值升级**：新增 `crates/adapter-kook/src/card_widgets.rs`，战局面板加入红蓝可视化 HP/SAN 进度条与双行控制台；严格遵守 KOOK 每个 `action-group` 最多 4 按钮、单卡最多 50 模块约束。
- **未开团频道白名单权威校验**：`KookClient` 新增 `channel_guild_cache` + `GET /api/v3/channel/view` 权威反查，确保在未建房频道点击置顶“一键开团”按钮同样严守 `ALLOWED_GUILD_IDS` 白名单。

## [0.3.0] - M1 DeepSeek 优化 + KOOK 语音开黑版
### 本次改动
- DeepSeek 客户端新增 429/5xx 晚高峰指数退避重试与 `Retry-After` 支持，单次 60s 超时不级联锁房，兼容显式 `null` 用量字段；优化首回合消息序列保证静态 System 前缀稳定命中缓存；新增 `usage_logs` 表与 `/cost` 消费/缓存命中率看板。
- 新增两把安全锁：`ALLOWED_GUILD_IDS` / `ALLOWED_CHANNEL_IDS` 亲友服白名单（含按钮回调权威 `room.guild_id` 校验与白名单开启时私聊隔离）+ `DAILY_MAX_LLM_CALLS` / `DAILY_MAX_CNY` 每日预算软熔断（覆盖主推演、后台摘要与多房并发）。
- 新增语音开黑双叙事档位 `/style voice`（100~180 字快扫卡）与 `/style prose`。
- KOOK 推演耗时前先发“⏳ DM 正在翻阅模组、推演本轮剧情…”占位卡，出卡前自动清理，优雅退出时等待在途事件收尾；秘密纸条原子落库 `whispers` 表，支持私信 → `temp_target_id` 仅本人可见临时消息 → 公屏提醒 + `/whisper` 私有补领三级兜底（长文本自动分片，永不泄露公屏）；KOOK API 增加 429 错峰重试。
- 新增实时团房主控场指令：`/afk`（暂离不卡自动结算）、`/leave`（退团）、`/gm hp|san|add|rm|clue|delclue` 与 `/reset` 权限管控（仅限开团房主与 `ADMIN_USER_IDS`，并在 `Resolving` 期间拒绝并发改写防覆盖）。

### 线上紧急 10 秒回滚命令（云端已备份旧二进制时）
```bash
mv -f /usr/local/bin/kook-tavern-dm.prev /usr/local/bin/kook-tavern-dm
systemctl restart kook-tavern-dm.service
systemctl is-active kook-tavern-dm.service
```

## [0.2.0] - M0.5 工程化底座重构
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
