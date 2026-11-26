# 🍺 KOOK AI 群酒馆 · 多人实时 AI 跑团引擎 (`kook-tavern-dm`)

[![License: AGPL v3](https://img.shields.io/badge/License-AGPL_v3-blue.svg)](./LICENSE)
[![Rust](https://img.shields.io/badge/Rust-2024_Edition-orange.svg)](https://www.rust-lang.org/)
[![CI](https://github.com/shu1yue/kook-tavern/actions/workflows/ci.yml/badge.svg)](https://github.com/shu1yue/kook-tavern/actions)

> 面向 **1~6 人亲友实时团** 的原生群聊 AI TRPG / 群酒馆 DM 引擎：纯 Rust 单二进制零外部依赖（常驻仅 **~16 MB 内存**），内置 **KOOK 卡片原地刷新故事流**、**5 子页置顶悬浮角色卡 HUD**、**4 层多人长剧本抗失忆引擎**、**多规则确定性检定（`2d6` / `D&D d20` / `CoC d100`）**、**SillyTavern 角色卡与世界书导入** 及 **内置 AI 剧本工坊**。
>
> *A lightweight, single-binary Rust multiplayer AI TRPG & Group Tavern DM engine for KOOK — featuring cohesive live story cards, a spoiler-safe floating player HUD (with Picture-in-Picture & HMAC whisper isolation), a 4-layer long-campaign memory engine, deterministic dice/act progression, and SillyTavern card/lorebook import. Licensed under **GNU AGPL-3.0**.*

## 🖼️ 界面与交互一览

<p align="center">
  <img src="./docs/screenshots/kook_live_story_card.jpg" alt="KOOK 原生连贯剧情卡与私密纸条/半路空降前情包" width="100%" />
</p>

<p align="center">
  <img src="./docs/screenshots/floating_hud_showcase.jpg" alt="5 子页个人实时悬浮 HUD（角色卡、SVG 人物关系星图、节点地图与封卷回忆录）" width="100%" />
</p>

---

## ✨ 核心亮点

- **📖 像翻小说一样的 KOOK 原生故事流**
  - 剧情卡内嵌实时小队栏、行动进度与快捷按钮，支持“点 ①②③ 按钮”或“直接说人话”（括号 `(...)` / `//` 自动视为场外 OOC 吹水）；
  - 推进新回合时自动通过 `message/update` 收起旧卡片按钮（且活跃卡片 `msg_id` 持久化在 SQLite，**重启不丢追踪**）；
  - 内置 KOOK `40011` 审核词自愈柔化与保底交互卡机制，拒绝吞回合、不断档。
- **🪟 5 子页个人实时悬浮 HUD（支持真·置顶画中画）**
  - 基于纯 Rust **HMAC-SHA256** 的个人只读凭证（结团 `/reset` 或离桌 `/leave` 自动失效），队友无法伪造偷看私密纸条；
  - 底部 5 格原生 App 导航：`🪪 角色`（血量/理智/五维/绝活/背包）、`🤫 纸条`（仅本人可见暗线密语 + 新纸条角标）、`🤝 人物`（交互式 SVG 人物关系星图，点头像交叉检索关联线索与密语）、`🎯 目标`（当前幕次主线 + 推荐行动 + 线索墙）、`📖 百科`（全局秒搜 + 互动节点地图 + 严格零剧透 `player_visible` 常识词典 + 历史前情总览）；
  - HTTPS 下支持 Chrome/Edge **Document Picture-in-Picture 真·永远置顶浮窗**，边开麦边挂在 KOOK 旁边。
- **🧠 4 层多人长剧本抗失忆引擎**
  1. **动态章节回忆录（Append-Only 永不覆写）**：借鉴 SillyTavern Memory Books，每满 6 轮封存 1 张带关键词的不可变章节卷，杜绝“传话筒式越压越糊”；
  2. **Letta 式契约与未解伏笔便签**：每次封卷独立维护当前未完成承诺、倒计时与悬念；
  3. **仅 DM 可见的暗线密语总账**：DM 牢记之前私发给不同队友的暗线底牌，多人长团不串戏、不公屏泄密；
  4. **线索智能打捞**：常驻最新 25 条线索，并按本轮行动/地点/人名滑窗召回早期相关物证。
- **🎬 半路空降无缝接团**
  - 朋友在开打后（`turn >= 1`）随时点选职业 / `/join` / `/char load` 空降进桌，系统会自动私发一份 **「🎬 欢迎半路空降 · 前情速递」**（含当前未解伏笔、近期封卷、刚刚发生的剧情、队友与一键悬浮 HUD 链接），同时 DM 会在下一轮自然把新队友写进镜头里。
- **🎲 后端确定性规则、自动转幕与成长体系**
  - 原生支持 **`2d6`（轻量三档）**、**`D&D d20`（优势/劣势、熟练加值、激励点、队友 `/assist` 协助、短休/长休、充能招牌技）** 与 **`d100`（克苏鲁风）**；
  - 后端按地图节点（`enter_nodes`）确定性兜底转幕，彻底告别大模型忘记转幕的问题，过幕自动触发全队 **Lv.+1 与 `/levelup` 成长点**；
  - 跨团个人角色库（`/char`）支持公平购点车卡与换本复用。
- **🛠️ 开源世界书商店 + AI 剧本工坊 + SillyTavern 导入**
  - 内置 6 套可复用的开放世界观底座（`worldbooks/`），支持群内 `/workshop` 引导式锻造新剧本，以及通过 `/gm import` 直接导入 **SillyTavern PNG 角色卡 / JSON 世界书**（内置 HTTPS 白名单 SSRF 防护与防 Zip Bomb 校验）。
- **💰 极致轻量与低成本**
  - 实测 2C2G Linux 服务器常驻内存仅 **~16 MB**；搭配 DeepSeek（开启前缀缓存优化），一场 26 轮完整跑团实测仅花费约 **¥0.40 RMB**，内置每日调用次数 & 金额双熔断与 `/cost` 实时账单。
  - 可选接入 **System One（Jev 潜意识反射层）** 做群聊 OOC 过滤、隐藏世界书语义召回与防泄密语义质检（不填 Key 则零开销关闭）。

---

## 📚 内置剧本与开源世界书

### 4 套开箱即用剧本（`scenarios/`）
| 剧本 ID | 名称 | 题材与特色 | 默认规则 |
|---|---|---|---|
| `ember_tavern` | **余烬酒馆与迷雾委托** | 蒸汽海港奇幻悬疑（3 幕 · 多结局） | `2d6` |
| `fog_manor` | **雨夜黑杉庄园** | 1926 暴风雪山庄哥特推理 / 轻量克系（3 幕） | `2d6` / 推荐 `d100` |
| `midnight_metro` | **午夜四号线 · 末班车怪谈** | 现代中式都市规则怪谈 / 微恐解谜（3 幕） | `2d6` |
| `sunforge_keep` | **剑湾晨钟 · 失落铸炉** | 经典 D&D 五幕中篇战役（含 4 职业招牌技） | `D&D d20` |

### 6 套内置开源世界书底座（`worldbooks/`）
`srd_sword_coast_fantasy`（剑湾奇幻）、`lovecraft_arkham_1920s`（1920s 阿卡姆调查）、`campus_rules_horror`（校园规则怪谈）、`cyber_kowloon_2099`（赛博九龙城）、`eastern_wuxia_jianghu`（国风武侠江湖）、`starship_frontier_colony`（星舰边疆殖民地）。

---

## 🚀 快速开始（3 种方式任选）

### 方式 1：本地零成本体验（无需 API Key，离线 Mock DM）
```bash
git clone https://github.com/shu1yue/kook-tavern.git
cd kook-tavern
cargo run -- local --mock-llm
```
> 在终端模拟器里可直接体验建卡、行动、掷骰、回档、多角色切换（`@角色名`）。

### 方式 2：Docker Compose 一键自部署（推荐）
```bash
git clone https://github.com/shu1yue/kook-tavern.git
cd kook-tavern
cp .env.example .env
# 编辑 .env，至少填入 KOOK_BOT_TOKEN 与 LLM_API_KEY
# 建议同时用 `openssl rand -hex 32` 生成一个 HUD_SIGNING_SECRET
docker compose up -d --build
```
服务启动后访问 `http://127.0.0.1:8787/healthz` 看到 `"status":"ok"` 即成功！
如需启用**浏览器原生置顶画中画悬浮 HUD**，建议在前面套一层 **Cloudflare Tunnel** 或 Nginx/Caddy HTTPS 反向代理，并将公网 HTTPS 地址填入 `.env` 的 `WEB_PUBLIC_BASE_URL`。

### 方式 3：单二进制部署到 Linux 服务器（2C2G 小鸡友好）
要求 `rustc >= 1.88`（支持 Rust 2024 Edition 与 let-chains）：
```bash
cp .env.example .env
# 编辑 .env 填入 KOOK_BOT_TOKEN、LLM_API_KEY、ALLOWED_GUILD_IDS、ADMIN_USER_IDS
CARGO_BUILD_JOBS=1 cargo build --release --locked -p kook-tavern-dm
./target/release/kook-tavern-dm kook
```
完整 `systemd` 托管、防断档原子更新与 SQLite 每日自动热备模板详见 [DEPLOY_ECS.example.md](./DEPLOY_ECS.example.md)。

---

## 🎮 常用指令速查

| 分类 | 指令（均支持中文别名） | 说明 |
|---|---|---|
| **开团与入桌** | `/menu` `/start <剧本ID>` `/roles` `/join` | 选本开团、选职业入桌或自定义车卡 |
| **推进与扮演** | 直接打字 / `/act <行动>` / `/go` / `/undo` | 提交行动、手动立刻结算、回档上一轮 |
| **检定与战斗** | `/check <属性>` `/roll 1d20` `/skill` `/assist <队友>` | 属性检定、自由掷骰、放招牌技、协助队友享优势 |
| **休整与成长** | `/短休`（`/rest short`） `/长休`（`/rest long`） `/levelup` | 恢复 HP/SAN/技能充能、分配过幕成长点 |
| **面板与浮窗** | `/me` `/hud` `/web` `/status` `/summary` | 私密角色卡、个人悬浮 HUD、全队实况大屏、前情摘要 |
| **创作与管理** | `/store` `/workshop` `/char` `/gm` `/export` `/reset` | 世界书商店、AI 工坊、跨团角色库、战报导出与结团 |

---

## 🧱 架构总览（8-Crate 六边形分层）

```text
kook-tavern/
├── crates/
│   ├── domain/          # 纯领域模型、骰子 RNG、世界书匹配、确定性数值/转幕 Patch（零 IO、零 unsafe）
│   ├── core/            # 抽象端口 Trait、显式 Collecting⇄Resolving 状态机、HMAC 浮窗令牌与命令编排
│   ├── infra-sqlite/    # SQLite WAL 存储、12 步版本号增量迁移、CAS 事务回档与 SillyTavern PNG/JSON 导入
│   ├── infra-llm/       # OpenAI 兼容传输层、4 层记忆本引擎、3 阶段 JSON 自愈解析、可选 Jev 决策门
│   ├── adapter-kook/    # KOOK v3 WebSocket、原地刷新活卡、频道级串行锁、40011 审核柔化与私密降级
│   ├── adapter-web/     # Axum 嵌入式只读大屏 + 5 子页个人悬浮 HUD + SSE 实时推送 + 代理感知限流
│   ├── adapter-cli/     # 本地多人终端模拟器
│   └── app/             # 主二进制入口与全链路集成测试套件
├── prompts/             # 外置热加载 DM/摘要/文风提示词（改完即生效，免重编译）
├── scenarios/           # 内置 4 套 YAML 剧本（支持 player_visible 防剧透拆分与 enter_nodes 自动转幕）
├── worldbooks/          # 内置 6 套开放世界书底座
├── Dockerfile           # 多阶段非 root 生产镜像
├── docker-compose.yml   # 一键自部署编排
└── DEPLOY_ECS.example.md
```

---

## 🛡️ 本地开发与质检门禁

```bash
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
本项目坚持严格的工程卫生规范：所有生产 `.rs` 源码文件均保持 **`<= 350` 行**、零 `unsafe`、零 Clippy 告警，详情见 [REVIEW_CHECKLIST.md](./REVIEW_CHECKLIST.md) 与 [CONTRIBUTING.md](./CONTRIBUTING.md)。

---

## 📜 开源协议（GNU AGPL-3.0）

本项目基于 **[GNU Affero General Public License v3.0 (AGPL-3.0)](./LICENSE)** 开源：
- 欢迎个人与亲友团自由下载、自部署、魔改剧本与二次开发；
- 如果你修改了本项目代码并**部署上线向他人提供网络服务（包括但不限于公开跑团 Bot 或 Web 服务）**，请遵循 AGPL-3.0 第 13 条，将你修改后的完整源代码同样以 AGPL-3.0 协议公开。
- 详见 [LICENSE](./LICENSE) 与 [CHANGELOG.md](./CHANGELOG.md)。
