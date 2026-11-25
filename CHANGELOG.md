# 变更日志与快速回滚手册（CHANGELOG）

> 每次版本上线前，用大白话记录“这次改了什么”以及“万一线上出状况怎么 10 秒回滚”。

## [1.8.0] - 4 层多人长剧本记忆引擎 + 半路空降私密前情包 + 反 AI 腔文风精修 + 活卡重启持久化
### 本次改动
- **4 层多人长剧本抗失忆引擎**（`crates/infra-llm/src/memory.rs`、`prompts.rs`、`prompts/summarizer.md`）：
  1. **动态章节回忆录（Append-Only 永不覆写）**：借鉴 SillyTavern Memory Books，每 8 轮封存 1 张带关键词的不可变章节卷，彻底杜绝“传话筒式反复压缩”把早期细节压没，同时 100% 兼容旧存档（自动冻结为卷 1）；
  2. **Letta 式契约与未解伏笔便签**：每次封卷独立刷新当前未完成承诺、悬而未决的冲突与倒计时，长团 50 轮也不忘债；
  3. **仅 DM 可见的暗线密语总账**（`list_room_whispers`）：DM 随时牢记之前给不同队友私发过的密语底牌，彻底避免多人长团忘掉每人秘密（严禁公屏泄露）；
  4. **线索智能打捞**（`format_focused_clues`）：保留最新 25 条线索，并按本轮行动/地点/道具采用滑窗字元召回最早期的相关线索，60+ 条线索也不丢第 1 轮关键物证。
- **半路空降无缝接团体验**（`party_helpers.rs`、`button_roles.rs`、`party.rs`、`char_vault.rs`、`hud_views.rs`、`hud.html`）：
  - 新朋友在 `turn >= 1` 中途通过选职业/快速入团/`/join`/`/char load` 空降时，仅本人私密收到一份 **「🎬 欢迎半路空降 · 前情速递」**（含前情摘要、刚刚发生的剧情、当前幕次主线、队友与关键人物、近期线索及一键打开悬浮 HUD 按钮），同时在公屏原地静默刷新队友剧情卡小队栏；
  - DM 导演层自动识别本轮首次登场的空降新人（`detect_drop_in_players`），在剧情里自然为 TA 安排合理登场镜头；
  - 悬浮 HUD `📖 百科 → 📖 前情` 顶部新增 **📜 历史前情总览**；同时修复悬浮 HUD 私密纸条按旧到新排序导致 `✨ 最新` 标在最旧一条的 Bug。
- **反网文套话 & 分剧本专属文风**（`prompts/style_prose.md`、`style_voice.md` 与 4 套内置 `scenarios/*.yaml`）：禁用高频 DeepSeek 套话，按蒸汽海港/哥特冷峻推理/中式通勤怪谈/剑湾 D&D 史诗差异化定制四套剧本 `dm_style`，搭配 `LLM_TEMPERATURE=1.15` 显著提升文笔质感。
- **重启防丢活卡持久化 + 每日自动热备**（SQLite 迁移 `0012_v18_live_story_cards`、`crates/adapter-kook/src/live_sync.rs`、`engine.rs`）：持久化活跃剧情卡 `msg_id`，发版重启后依旧能精准归档旧卡按钮并避免重复无意义 `message/update` 调用；ECS 新增每日凌晨 SQLite 在线热备（滚动保留 7 天）。
- **Gate 10 全仓代码健康体检与红队加固（修复 4 项 P0 + 10 项 P1）**：
  - **彻底阻断悬疑剧本剧透（P0）**：为世界书新增 `player_visible`（默认 `false` 故障关闭）显式字段，将《雨夜黑杉庄园》与《午夜四号线》的公开传单与 DM 真相彻底拆离，悬浮 HUD 百科绝不再泄露幕后真相或 DM 提示；
  - **修复 Cloudflare Tunnel 共享限流桶自拒服（P0）**：`extract_client_ip` 仅在回环代理下识别 `CF-Connecting-IP`，防恶意扫描拖垮全体玩家 HUD/SSE；
  - **修复中文高位字元召回截断与分隔符注入（P0）**：记忆检索改为保序去重（不再按码点字典序截断高位中文人名）、章节召回按命中分排序，并对 LLM 总结正文清洗保留分隔符防止破坏历史封卷；
  - **修复摘要微卷分裂与并发竞态（P1）**：修正后台摘要触发公式（每次完整封存 6 轮、按 `summary_upto_turn + created_at` 写 CAS 不再被正常推进打断）、为 KOOK 活卡操作加频道级串行锁防并发覆写、修复 HUD 地图当前点高亮（`active_node_id`）、修复 `不信任`/`友好` 态度配色判定、修复空降卡职业按钮路由（`rolemenu`）与降级链接保留、补全 40011 柔化覆盖并避免 `SilentOnSuccess` 误吞暂存提示。

## [1.7.4] - 精简私人酒馆首页文案（砍掉冗余技术细节，20 秒清爽看完）
### 本次改动
- 大幅精简 `crates/adapter-web/assets/landing.html`，去掉服务器规格、底层技术栈术语与冗长长文，保留「实时营业灯 + 4 张玩法简卡 + 3 步入桌 + 极简 3 行进度条」，给朋友看更轻松顺眼。

## [1.7.3] - 私人酒馆门面与开发进度展示首页（`GET /`）
### 本次改动
- 新增纯静态零数据库查询的私人好友圈项目展示与开发进度页（`crates/adapter-web/assets/landing.html`、`GET /`）：含实时 `/healthz` 营业绿灯、4 大玩法亮点、30 秒入坑指南、内联 SVG 系统架构图，以及 `v0.1 → v1.7.3` 阶段进度与实弹修 Bug 日记（带 `noindex, nofollow` 隐私保护，绝不暴露任何房间或玩家数据）。

## [1.7.2] - 极简 5 子页悬浮冒险者 HUD 重构（超薄顶栏 + 底部 5 格 App 导航）
### 本次改动
- **彻底告别信息拥挤**（`crates/adapter-web/assets/hud.html`、`hud_views.rs`）：将常驻顶栏瘦身为 46px 极薄状态条（仅保留实时灯、回合、迷你血量/理智与 `📌 浮窗`），屏幕 85% 以上空间留给正文；改为底部固定 5 格原生 App 导航，一页只专注回答一个问题：
  1. `🪪 角色`（血量/理智条、五维属性、绝活充能与背包行囊）；
  2. `🤫 纸条`（独立暗线密语时间线，新纸条到达亮紫点角标提醒）；
  3. `🤝 人物`（精简人物关系星图 + 默认收起的手风琴人物卡，点谁才展开 TA 的关联线索）；
  4. `🎯 目标`（当前幕次主线、本轮已绑定检定、参考行动方向 `suggested_actions` 与精简线索墙）；
  5. `📖 百科`（把全局秒搜框、迷你地图、零剧透常识词典、最近前情与势力号外收进独立参考手册）。

## [1.7.1] - KOOK 40011 敏感词拦截自愈防吞回合 + `/resend` 一键补发最新剧情卡
### 本次改动
- **KOOK `code=40011` 自愈防吞卡**（`crates/adapter-kook/src/sanitize.rs` 与 `dispatcher.rs`）：遇到 KOOK 平台自动审核拦截（如“持枪/枪声/炸毁”等现代武器词）时，先自动柔化为奇幻/蒸汽朋克表述重发完整剧情卡；若仍拦截则下发保留全部交互按钮与 HUD 的保底剧情卡，**彻底杜绝吞回合与丢按钮**。
- **Prompt 源头避敏**：`prompts/dm_system.md` 增加蒸汽/奇幻替代表述约束，减少源头触敏。
- **新增 `/resend`（`/补发` / `/重发`）指令**（`crates/core/src/commands/turn_support.rs`）：随时把数据库中已保存的最新一轮剧情卡连同可交互按钮重新锚定到频道底部。

## [1.7.0] - 个人实时悬浮冒险者 HUD（角色卡 + 人物关系星图 + 线索百科 + 密语实时推送）
### 本次改动
- **零依赖 HMAC-SHA256 个人签名令牌**（`crates/core/src/hud_token.rs`）：每个玩家拥有绑定 `(服务端密钥, room.web_token, channel_id, user_id)` 的 128-bit 专属只读浮窗凭证，队友无法用公屏大屏链接伪造偷看私密纸条，结团 `/reset` 或 `/leave` 离桌后立即失效。
- **4 标签暗黑酒馆风悬浮 HUD**（`/hud/{channel_id}`、`crates/adapter-web/src/hud_views.rs` 与 `assets/hud.html`）：
  - **🪪 角色**：常驻血量/理智条、本轮已绑定检定/行动横幅、五维属性、招牌绝活充能、背包行囊、以及带新消息红点闪烁的**仅本人可见专属秘密纸条箱**；
  - **🤝 关系（专治名字太多看晕）**：交互式 SVG 人物关系星图（按态度自动连色：🟢信任/🟡中立/🟠警惕/🔴敌对），点击任意 NPC 或队友头像瞬间弹出档案，并自动交叉关联相关线索、密语与势力号外；
  - **🔎 线索与零剧透百科**：当前幕次主线目标、已解锁关键节点、线索墙、以及严格仅展示公开常识（`always_active`）的零剧透世界词典；顶部全局搜索框支持秒搜任意人名/道具/线索/常识；
  - **🗺️ 地图与防忘词速览**：缩小版互动行动地图 + 最近 6 轮剧情提要；
  - **📌 置顶悬浮窗**：HTTPS 下支持 Chrome/Edge 原生 **Document Picture-in-Picture 真·永远置顶浮窗**，普通 HTTP 自动降级弹出独立窄窗。
- **KOOK 一键入口**：在 `🪪 我的角色卡`（`/me`）私密卡新增原生外链按钮 **「🪟 悬浮角色卡 HUD」**，并支持 `/hud`（`/浮窗`）快捷指令直出专属链接。

## [1.6.3] - 空响应自动重试 + 报错保护活卡不收按钮 + 检定/掷骰即时私密反馈
### 本次改动
- **上游偶发空内容自愈重试**：`crates/infra-llm/src/transport.rs` 与 `wire.rs` 记录 `finish_reason`，遇 HTTP 200 空内容自动退避重试（末次重试放宽 `response_format`），大幅降低 DeepSeek 高峰期偶发空回包中断体验的概率。
- **报错绝不误收当前剧情卡**：`crates/adapter-kook/src/dispatcher.rs` 区分 `HudSync::Skip` 与 `HudSync::Live`，报错卡片不再把上一张正常剧情卡的按钮误撤掉，且报错卡本身自带 **「⚡ 立刻结算」** 一键重试。
- **检定 / 掷骰 / 放技能即时爽感反馈**：`events.rs` 将 `check / roll / skill` 改为原地刷新剧情卡 + 底部立即弹出**仅本人可见**的轻量骰子结果提示，并修正实时栏重复 `🎲` 图标。

## [1.6.2] - DeepSeek JSON 容错自愈（修复多余闭合括号/尾逗号 + 严禁原始 JSON 泄密）
### 本次改动
- 升级 `crates/infra-llm/src/parser.rs` 为 3 级容错解析：① 严格 JSON；② 括号栈自愈修复器（自动剔除大模型偶发吐出的多余 `]]` / `}}` 与尾逗号，完整恢复全部剧情、背包、线索、NPC 与私密纸条）；③ 零原始 JSON 泄密兜底（即便极端截断也只提取 `scene_title` 与 `narrative`，绝不把含 `private_whispers` 的原始 JSON 吐到公屏）。

## [1.6.1] - 面板一键「🚪 退团离桌」与二次确认「🛑 结束本局」按钮
### 本次改动
- 在 `🎛️ 面板/设置` 浮层新增 **「🚪 退团离桌」**（队员单独体面离桌并原地刷新小队栏）与 **「🛑 结束本局」**（房主/管理员私密二次确认防误触，两下秒清桌重开，顺带提供「📜 先发战报墙」留档入口）。

## [1.6.0] - 剧场级卡片连贯流 + 原地刷新（KOOK `message/update`）+ 无打断临时浮层
### 本次改动
- **📖 公屏纯净故事流（自动收起旧卡按钮）**：新增 `crates/adapter-kook/src/live_cards.rs` 与 KOOK `POST /api/v3/message/update` 原地更新能力。每当推进到新回合发出新剧情卡时，自动把上一张旧剧情卡的交互按钮撤掉变为纯净小说页，公屏永远只有**最新一张剧情卡**带操作区，彻底告别翻错旧卡与视觉割裂。
- **🎭 开局与选职业合二为一**：第 0 幕开场剧情卡直接内嵌 4 个预设职业绿色按钮与「👥 当前小队」实时栏；点选职业时直接**原地刷新**开局卡亮出小队名单，不再连发「选职业卡」「入团成功卡」打断开场阅读。
- **📌 检定 / 掷骰 / 放技能 / 行动进度原地刷新**：回合内执行 `/check`、`/roll`、`/skill`、`/assist`、`/levelup` 或多人等待行动时，直接**原地刷新最新剧情卡底部的「📌 本轮动态」状态条**，个人回执以仅点击者可见的轻浮层弹出，绝不往公屏砸碎卡片。
- **🪟 面板与选职业菜单改为轻浮层**：点 `🎛️ 面板/设置`（`StatusBoard`）与 `🎭 换职业`（`RolePicker`）默认以 `temp_target_id` 仅对本人弹出，不打断队友阅读、刷新即净。

## [1.5.0] - 开源世界书商店 + 5 步引导式剧本工坊 + 零记忆私密角色卡快捷按钮
### 本次改动
- **📦 开源世界书商店（`/store` & `📦 世界书商店`）**：内置 6 套 CC0 / SRD-Inspired 开源世界观底座包（`worldbooks/*.yaml`：古典西幻剑湾城邦、1926 阿卡姆克苏鲁、九龙霓虹 2099 赛博城、青衫剑雨武侠江湖、午夜十三楼校园规则怪谈、深空孤舰星际殖民），每套均含 6 条深度世界书、多方势力与 **3 个防卡壳现成故事钩子**，在 KOOK 卡片里一键预览、一键带入工坊。
- **✍️ 5 步引导式剧本创作工坊（`/workshop` & `✍️ 引导创作工坊`）**：
  - 新增 SQLite 增量迁移 `0011_v15_workshop_drafts`，按用户隔离保存个人草稿与 AI 锻造冷却；
  - 彻底摆脱三段式束缚：按钮自由切换 **3 幕短篇（1晚）/ 5 幕中篇（2~3晚）/ 6 幕史诗** 与 **`d20` / `2d6` / `d100`** 规则体系，并支持追加最多 6 条私房世界书（`/workshop lore`）；
  - **🔥 AI 锻造与预览**：融合所选开源世界书 + 私房设定 + 脑洞生成完整可玩的 D&D 多幕模组（含地图节点、势力、4 个带招牌技的职业与 3~4 结局），受全局每日预算熔断与 60 秒冷却保护；
  - **🚀 一键上架社区剧本库（零重启热重载）**：自动隔离写入 `scenarios/community/`（上限 32 个，严禁覆盖官方内置剧本），上架即刻全服可 `/start` 开玩；支持管理员 `/workshop delete <ID>` 下架社区本及 SSRF 白名单加固的 `/workshop import <HTTPS链接>` 远程导入 SillyTavern JSON/PNG 资产；工坊卡片在频道内按用户临时卡发送不刷公屏。
- **🪪 `/me` 个人私密角色卡零记忆按钮补强**：在仅本人可见的 `/me` 角色卡底部自动附带可点击的 **✨ 施放招牌技** 与 **⬆️ 一键升级（+2血/理智 / 招牌技+1充能）** 快捷按钮，跑 D&D 复杂团再也不用死记命令。

## [1.4.0] - 经典 D&D 多规则引擎 + 招牌技与营地休整 + 过幕升级 + 跨团角色库 + 五幕中篇示范本
### 本次改动
- **🐉 多规则检定引擎（`d20` D&D / `2d6` 三档 / `d100` CoC）**：新增 `crates/domain/src/dnd.rs` 与 `dnd_ops.rs`，支持 `/rule d20|2d6|d100` 随时切换本桌规则；`d20` 原生支持 `DC`（默认 12，可写 `dc15`/`dc20`）、`adv` 优势 / `dis` 劣势双骰、`Nat 20` 天然大成功、`Nat 1` 天然大失败，以及熟练技能自动加值（`d20 +2` / `d100 +15%` / `2d6 +1`）。
- **🌟 激励点 + 🤝 队友协助 + ✨ 招牌技能充能 + 🏕️ 营地休整 + ⬆️ 过幕升级**：
  - `/inspire <角色名>` 或 DM JSON `grant_inspiration` 赏赐激励点，检定带 `inspire` 直接触发优势双骰；
  - `/assist <队友名>` 为队友下一次检定挂上自动优势；
  - `/skill <编号或名> [目标]` 原子扣减招牌技充能并绑定本轮高光演出；
  - `/rest short`（短休回血+短休技）/ `/rest long`（房主长休全满，带防刷冷却）；
  - 每次推进新幕次（`advance_act`）全队自动升级并获得 1 成长点，发 `/levelup hp|<属性>|skill <技能>|charge` 四选一强化，且完整支持 `/undo` 回档。
- **🪪 自由车卡与跨团个人角色库（`/char`）**：新增 SQLite 增量迁移 `0010_v14_dnd_rules_and_chars` 与 `saved_characters` 表（每人最多 8 张），强制公平购点（单项 `-1..=+3`、总和 `≤ +3`），支持个人背景与仅自己和 DM 可见的 `personal_secret`（Web 端严格脱敏不泄露），支持跨团 `/char load` 并防数值/充能膨胀。
- **🏰 全新第 4 套 5 幕 D&D 中篇战役本《剑湾晨钟 · 失落铸炉》**：`scenarios/sunforge_keep.yaml`（`d20` 规则、5 幕主线、10 节点 SVG 地图、4 幕后势力、14 条世界书、4 个带 2 项招牌技的 D&D 职业、4 大结局，2~3 晚标准体量）。
- **🖥️ 卡片与伴生大屏升级**：`/me` 私人卡与只读 Web 看板同步展示规则徽章、等级、激励星标、熟练技能与招牌技剩余充能；兼容静态工坊频道 `tavern:menu` / `tavern:endings` 按钮回调。

## [1.3.1] - 阿里百炼 System One 正向极性 OOC 实网校准
### 本次改动
- 根据阿里云实网压测结果，将 Jev OOC 吹水门卫问法校准为正向极性 `is_in_game_action`（消息置顶），实网场外闲聊区分度由 `0.50` 跃升至 `0.89~0.95`，戏内行动保持 `0.00~0.04`，响应延迟 ~140–215ms；新增 Jev 启动就绪脱敏日志。

## [1.3.0] - 可选阿里百炼 System One / Google Decisions Jev 潜意识反射层
### 本次改动
- 新增可选 `DecisionGate` 决策反射层（未配置 `JEV_API_KEY` 时零开销静默回退纯 Rust）：① 群聊 `Roleplay` 模式场外吹水门卫（KOOK 占位卡前三态预检零闪烁、零重复调用）；② 隐藏世界书语义补漏（与关键词召回取并集，100% 保持 DeepSeek Prefix Cache）；③ DM 回复语义质检（消除 `"幸好没受伤"` 误报 + 拦截 `private_whispers` 泄密触发 `redacted_narrative` 重写）。
- 过 Gate 7 红队体检：支持显式 `null` 容错、拒绝越界概率、禁用 3xx 重定向防自定义头泄密、总超时硬预算与 256 KiB 流式上限。

## [1.2.0] - 借鉴《文明与地图》暗流号外、轮回残响、聚光灯守护与 6 节点交互沙盘地图
### 本次改动
- 新增幕间暗流号外（`world_bulletin`）、同服轮回残响（`world_echoes`，SQLite v8/v9 迁移与同频道 `/undo` 精准回滚）、多人聚光灯公平追踪器、预算受控一次性状态一致性补齐校验。
- Web 伴生大屏升级为 6+ 节点 SVG 交互沙盘地图（含玩家/NPC 站位与未探秘点 `？？？` 防剧透）、分档编年史大事记与纯前端 HTML5 Canvas PNG 战报长图导出。
- 三大既有剧本全面升级补齐地图节点、幕后势力与导演必达大纲节点。

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
