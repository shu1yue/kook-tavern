# 🤝 贡献指南（Contributing）

欢迎提交新剧本、世界书底座、Prompt 文风调优、Bug 修复或新聊天平台适配器（如 Discord / QQ / Telegram）！

## 1. 最简单的贡献方式：提交剧本或世界书（零代码）
- **新增剧本**：在 `scenarios/` 下新增 `<scenario_id>.yaml`（可参考 `scenarios/ember_tavern.yaml` 或 `scenarios/sunforge_keep.yaml`）。
  - 注意为向玩家展示的公开世界书条目显式标注 `player_visible: true`，幕后真相与 DM 提示请保持默认 `player_visible: false`（防止在悬浮 HUD 百科中剧透）。
  - 建议为第 2 幕及之后的 `acts` 配置 `enter_nodes`，让引擎在队伍踏入对应地图节点时确定性自动转幕并触发过幕升级。
- **新增世界书底座**：在 `worldbooks/` 下新增 `<pack_id>.yaml`（可参考现有 6 套开放世界书）。
- 提交前请运行 `cargo test --workspace`，系统会自动校验所有内置 YAML 的 Schema、地图连通性与防剧透规则。

## 2. 提交 Rust 代码规范
1. **单文件克制**：所有生产代码文件（`crates/*/src/**/*.rs`）保持 **`<= 350` 行**，保持六边形分层（`domain` 零 IO，`core` 不依赖具体传输/存储实现）。
2. **零告警门禁**：提交 PR 前确保以下命令全部通过：
   ```bash
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace
   ```
3. **隐私与安全红线**：严禁提交任何 `.env`、真实服务器 IP、频道/服务器 ID、API Key 或用户聊天记录；任何新增 Web/HUD 字段必须验证不泄露队友私密纸条、个人秘密与未解锁剧本真相。

## 3. 协议约定
向本项目提交的所有贡献均默认以与本项目相同的 **[GNU AGPL-3.0](./LICENSE)** 协议发布。
