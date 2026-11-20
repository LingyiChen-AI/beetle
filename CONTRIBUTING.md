# 贡献指南

## 开发环境

Node.js 18+、pnpm、Rust stable、Python 3.10+；Windows 需 WebView2。

```bash
pnpm install
pnpm tauri dev
```

## 提交前检查

```bash
cd src-tauri && cargo test
pnpm vitest run
pnpm exec tsc --noEmit
python -m pytest src-tauri/resources/skills/_tests -q
```

## 提交信息

使用中文，只写实际改动，一次提交做一件事。

## 新增技能（技能中心）

在 `src-tauri/resources/catalog/<技能名>/` 下新建目录：

```
<技能名>/
  SKILL.md        技能说明（Agent 读取的入口）
  catalog.json    技能元信息
  scripts/        脚本（可选）
  _shared/        共用代码（可选）
```

`catalog.json` 字段：

| 字段 | 说明 |
|---|---|
| `name` | 技能标识，与目录名一致 |
| `display_name` | 界面显示名 |
| `description` | 一句话说明用途与触发场景 |
| `version` | 版本号，改动技能内容时递增 |
| `requires` | 依赖列表，如 `python`、`node`、`lark_cli`、`git` |
| `config` | 关联的配置命名空间（无则省略） |

可参考现有的 `src-tauri/resources/catalog/bug-hunt`。机器人内置技能放在 `src-tauri/resources/skills/`，并在 `_tests` 下补测试。

## 发版

1. 把 `src-tauri/tauri.conf.json`、`src-tauri/Cargo.toml`、`package.json` 里的版本号改成同一个新版本（如 `0.2.0`），提交。
2. 打标签并推送：`git tag v0.2.0 && git push origin v0.2.0`。
3. GitHub Actions（`.github/workflows/release.yml`）会自动打 Windows / macOS 安装包，签名后发布到 Release，并生成 `latest.json`。
4. 已安装的客户端每 30 分钟检查一次 `latest.json`，发现新版本后在界面顶部提示「立即更新」。

更新包签名用的私钥放在仓库 Secrets：`TAURI_SIGNING_PRIVATE_KEY`、`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`；公钥写在 `tauri.conf.json` 的 `plugins.updater.pubkey`。私钥丢失后已安装的客户端将无法验证新版本，务必备份。
