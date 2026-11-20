---
name: issue-record
description: 把一条 bug 分析建成项目仓库（Gitee / GitHub）的 issue；同标题的 open issue 已存在就追加评论。找到问题、且本项目记录方式勾选了「建 issue」时才用。
---

# 建 issue

报告 markdown 和「飞书文档记录」共用同一份（在当前目录的 `reports/` 下，第一行 `## <bug 标题>` 作为 issue 标题，其余为正文），结构：

```
## <bug 标题，一句话>

**复现步骤**

1. ...

**修复建议**

1. ...（文件路径:行号）

**数据依据**：<关键查询结果的一句话摘要>

**提出人**：<姓名>

**提出时间**：<YYYY-MM-DD HH:MM>

**代码版本**：<分支> @ <提交号>
```

然后执行：

```
{{PYTHON}} {{SKILL_DIR}}/scripts/create_issue.py --project <项目名> --file reports/<文件名>.md
```

- 平台按项目仓库地址自动选（gitee.com → Gitee，github.com → GitHub），仓库、token 环境变量名都取运行时配置（环境变量 `BEETLE_RUNTIME`，客户端自动注入），不要自己传。
- 只想看本项目的记录方式、token 有没有配置：加 `--plan`（不联网、不建 issue）。
- 输出「已创建 issue：<链接>」或「已在 <链接> 追加评论」才算成功，把链接写进最终报告【记录】。
- 退出码 3（未配置 token）或其它失败：**不要换参数重试**，也不要自己找 token；不影响写飞书文档，在【记录】里如实写「issue 未创建：<原因>」。
- token 只在脚本内部使用，不要读取、打印、写进报告或文档。正文里不出现口令、token、手机号、买家昵称或收货信息。
