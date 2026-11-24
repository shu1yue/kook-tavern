# 🍺 KOOK AI 群酒馆 / 轻量文字冒险引擎 (`kook-tavern-dm`)

面向 1~6 人亲友实时团的 **KOOK 平台 AI DM 群酒馆与轻量跑团引擎**（Rust Cargo Workspace 六边形分层架构，单二进制零依赖部署，内置 SQLite 版本迁移、显式并发状态机、外置可编辑 Prompt 与富文本卡片交互）。

## 🧱 Workspace 架构总览

```text
kook-tavern-dm/
├── crates/
│   ├── domain/          # 纯领域模型、骰子 RNG、世界书匹配、数值 Patch 校验（零 IO）
│   ├── core/            # 抽象端口 Trait、显式 Collecting⇄Resolving 状态机、命令编排
│   ├── infra-sqlite/    # SQLite WAL 持久化、版本号增量迁移、CAS 事务回档与 YAML 剧本加载
│   ├── infra-llm/       # OpenAI 兼容传输层、外置 Prompt 热加载、容错 JSON 解析、Mock DM
│   ├── adapter-kook/    # KOOK API v3 WebSocket、富文本卡片、交互按钮与错峰私信/临时消息降级
│   ├── adapter-cli/     # 本地多人终端模拟器（支持 @切角色 与中文全角标点）
│   └── app/             # 主二进制入口（编译产出 target/release/kook-tavern-dm）
├── prompts/             # 外置可编辑 DM 提示词（改完即生效，无需重编译！）
├── scenarios/           # YAML 剧本与世界书（内置《余烬酒馆》《黑杉庄园》）
├── REVIEW_CHECKLIST.md  # 代码审查红队硬性清单
├── CHANGELOG.md         # 变更日志与 10 秒快速回滚手册
└── DEPLOY_ECS.example.md # 云端部署脱敏模板
```

## 🚀 快速上手

### 1. 本地零成本体验（离线 Mock DM）
```bash
cargo run -- local --mock-llm
```

### 2. 接真大模型（推荐 DeepSeek）本地试玩
```bash
cp .env.example .env
# 填入 LLM_BASE_URL / LLM_API_KEY / LLM_MODEL
cargo run -- local
```

### 3. 上线 KOOK 机器人
在 `.env` 填入 KOOK 开发者中心（WebSocket 模式）的 `KOOK_BOT_TOKEN` 后：
```bash
cargo build --release -p kook-tavern-dm
./target/release/kook-tavern-dm kook
```

## ✍️ 零代码调教 DM 文风与自定义剧本
- **调教 DM 口吻/字数**：直接编辑 [prompts/dm_system.md](prompts/dm_system.md)、[prompts/style_prose.md](prompts/style_prose.md) 或 [prompts/style_voice.md](prompts/style_voice.md)，下个回合立刻生效，无需重新编译！
- **新增剧本与世界书**：往 `scenarios/` 放入 `.yaml` 文件即可。

## 🛡️ 本地开发与质检门禁
```bash
make check   # 依次执行 cargo fmt --check、clippy -D warnings、workspace 全量测试
```
