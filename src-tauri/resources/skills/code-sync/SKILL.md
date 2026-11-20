---
name: code-sync
description: 把本群负责的项目同步到最新 release 分支（只读分析用），输出代码目录、分支、提交号和项目规则文件。
---

# 代码同步

每次排查都要先同步，不能用旧代码。运行时配置（环境变量 `BEETLE_RUNTIME`，客户端自动注入）里的 `projects` 登记了本群负责的项目，`code_root` 是代码缓存目录。

```
{{PYTHON}} {{SKILL_DIR}}/scripts/sync_code.py <项目名>
{{PYTHON}} {{SKILL_DIR}}/scripts/sync_code.py <项目名> --branch release/V3.2
```

- 默认按项目的 `branch_glob`（默认 `release/*`）取版本号最大的分支；`--branch` 指定分支。
- 每次都会 fetch 并强制切到最新分支，丢弃该目录里的本地改动，所以**只读代码，不要在里面改文件、commit、push**。
- 输出里的「目录」是代码所在路径（每个分支一个独立目录 `<项目>@<分支>`，不要去 `<code_root>/<项目>` 本身读代码），「先读」是项目规则文件（如 AGENTS.md），先读它再按 bug 现象搜相关代码。记下分支名和提交号，写进最终报告的依据。
- 别的群正在同步同一个项目时会稍等（最多 10 分钟）；git 不会弹凭据窗口，仓库凭据失效会直接报错，把报错写进报告请机器人主人处理。
- 项目名不在登记里会报「未登记的项目」并列出可用项目，不要自己猜仓库地址。
- 代码审查时用 `--pr <编号> --sha <head 提交>` 检出 PR 的最新提交（Gitee / GitHub 的 `refs/pull/<编号>/head`），目录是这个 PR 独立的 `<项目>@pr-<编号>`，同样只读。
  输出里的「提示」说明 PR 在拉取元数据之后又更新过，按提示判断看的是哪个提交。

```
{{PYTHON}} {{SKILL_DIR}}/scripts/sync_code.py <项目名> --pr 29 --sha 1a2b3c4
```
