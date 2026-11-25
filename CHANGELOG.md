# 变更日志与快速回滚手册（CHANGELOG）

> 每次版本上线前，用大白话记录“这次改了什么”以及“万一线上出状况怎么 10 秒回滚”。

## [1.0.0] - 个人专属角色卡 `/me` + M4 内容管线（AI 一键生本 / SillyTavern 导入 / 零重启热重载）
### 本次改动
- **🪪 仅本人可见的个人角色卡（`/me` 与剧情卡按钮）**：点击剧情卡底部「🪪 我的角色卡」或发送 `/me`，在 KOOK 频道原地弹出**仅自己可见的专属卡片**（队友完全不可见、无需切去私信箱），一屏整合个人血条/理智条、五维属性、特质、完整背包、本轮已提交行动/检定与最近 5 条专属秘密纸条。
- **🔄 零重启热重载（`/reload`）**：`FsScenarioCatalog` 升级为 `RwLock` 原子热切换，新增强类型 Schema 校验器 `crates/domain/src/validate.rs`，管理员发 `/reload` 即可实时刷入新剧本；若存在坏 YAML 会安全中止并保留原线上剧本零宕机。
- **✨ AI 一键生本（`gen-scenario`）**：新增 `kook-tavern-dm gen-scenario "<脑洞>"` 命令，调用 DeepSeek 自动生成完整 v2 多幕多结局剧本（4 职业+五维+3 幕+3 结局+世界书），经 Schema 强校验后原子写入 `scenarios/`，默认防误覆写。
- **🃏 SillyTavern 零依赖导入（`import-st`）**：新增纯 Rust 零外部新依赖的 `kook-tavern-dm import-st <file.png|json>`，原生解析 PNG `ccv3`/`chara` Base64 元数据块（含 `tEXt` 与 zlib `iTXt`）以及 V1/V2/V3 角色卡与独立 World Info JSON，一键转换为合规 v2 剧本。

## [0.6.0] - M3 轻量五维检定 + 幕次进度多结局 + 结局图鉴与三主题大屏
### 本次改动
- **零门槛五维属性与三档确定性检定**：新增 `crates/domain/src/stats.rs`（`体魄/敏捷/侦查/交涉/灵感`），为三大剧本共 12 个预设职业配齐差异化属性加值；新增 `/check <属性> [原因]` 及剧情卡动态推荐检定按钮，后端确定性计算 `2d6 + 属性` 并裁决 ❌ 失败（≤6）/ ⚠️ 代价成功（7~9）/ ✨ 完全成功（≥10）。
- **幕次进度树与自动多结局收束**：三大剧本全面升级为 v2 多幕结构（各 3 幕主线 `acts` + 3 大明确结局 `endings`）；SQLite 新增增量迁移 `0007_m3_stats_acts_endings`（兼容历史库与 `/undo` 结局改命）；后端权威校验结局 ID 并设早退保护，达成结局时自动弹出史诗金色剧终卡与专属操作条。
- **本服「🏆 结局图鉴」成就系统**：新增 `ending_unlocks` 表、`/endings` 指令与控制台按钮，自动记录本服务器解锁的每个结局与耗时回合（未解锁结局严格遮罩为 `？？？` 零剧透）。
- **Web 伴生大屏视觉大升级**：新增三剧本专属动态主题色（`ember` 琥珀炉火 / `fog` 雨夜幽蓝 / `metro` 末班地铁霓虹绿），顶部增加幕次进度条、关键进展节点徽章与终局荣耀横幅，侧栏同步展示五维属性徽章与本剧本结局图鉴（继续严守全只读、全 `textContent` 防 XSS、零敏感字段泄露）。

## [0.5.1] - 真模型实弹试跑体验打磨
### 本次改动
- **强化秘密纸条防剧透隔离**：更新 `prompts/dm_system.md`，明文禁止 AI 将 `private_whispers` 中的私密情报同步泄露进公开 `narrative`、公开 `new_clues`、`status_note` 或 `suggested_actions`，真正守住私密纸条的悬疑与博弈感。
- **修复单人开荒“幻觉队友”**：要求 AI 严格按【队员状态】实际人数描写视角与建议，1 人独行测试时不再凭空虚构同行同伴。
- **优化 KOOK 行动按钮文案适配**：要求 DM 输出 6~14 字精炼动作短语作为 `suggested_actions`，同时将 KOOK 剧情卡按钮截断阈值从 13 字提升至 16 字，告别按钮末尾省略号。
- **细节修整**：更新 `/join` 入团提示以匹配零代码直接打字/点按钮玩法，补全本地 CLI 模拟器战局面板的 NPC 状态显示与多参 `:btn` 支持。

## [0.5.0] - M2 内嵌只读伴生 Web 看板大屏
### 本次改动
- 新增 `crates/adapter-web`（精简 `axum 0.8` + SSE，默认监听 `0.0.0.0:8787`，严守绝不触碰 `8080/3000/6099`），与 KOOK 机器人同进程、单二进制运行。
- **纯只读安全设计**：仅开放 4 个 `GET` 路由（`/healthz`、`/room/{channel_id}`、`/api/room/{channel_id}`、`/api/room/{channel_id}/events`），常量时间校验房间随机 `web_token`（错误或未知房间统一 `403` 防枚举）、每 IP 限流（120 req/min，有界 2048 桶）、全局 SSE 上限 64 连接、全套安全响应头（`CSP` / `DENY` / `no-referrer` / `no-store`），API 严格脱敏（绝不输出 `web_token`、`gm_user_id`、`guild_id`、`user_id` 或秘密纸条）。
- **暗黑奇幻风实况大屏**：内嵌单文件响应式网页（零外部 CDN、全量 `textContent` 防 XSS），接入 `BroadcastEventBus` 实时 SSE 推送，KOOK 里提交行动/结算/GM 改血量后网页 1 秒内无刷新同步更新。
- KOOK 侧新增 `/web` 指令与战局控制台 **`🌐 Web 看板`** 按钮，一键甩出当前房间专属只读大屏链接。

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
