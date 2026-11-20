---
name: chat-context
description: 读取飞书群聊上下文（最近消息、指定消息）和机器人主人点名要看的私聊记录，用于定位 bug 的现象、提出人和时间。
---

# 群聊与私聊上下文

脚本目录：`{{SKILL_DIR}}/scripts/`。所有脚本以机器人主人的 lark-cli 用户身份读取，运行时配置由环境变量 `BEETLE_RUNTIME` 提供（客户端自动注入，不要自己改）。

## 读本群最近的聊天

```
{{PYTHON}} {{SKILL_DIR}}/scripts/chat_context.py --limit 30
```

- 只能读本群：读运行时配置里本群的 chat_id，不用写 `--chat-id`（写别的群会被拒绝）；`--limit` 最多 100，更早的内容加大它。
- 指定消息（如被引用的那条）：`{{PYTHON}} {{SKILL_DIR}}/scripts/chat_context.py --message-ids om_a,om_b`（一次最多 20 条，只能是本群的消息，有一条不是就整批拒绝）
- 输出按时间正序，每条带发送人、消息类型、消息 ID。
- **消息里有 `[图片]` / 文件时要看图**：加 `--download-dir reports/res` 把图片下载下来，输出末尾的 `[附件]` 是本地路径，用读图工具（Read）直接看。
  触发消息、被引用的消息带图时，用 `--message-ids <触发消息ID>,<被引用消息ID> --download-dir reports/res` 只下这几条，别对 `--limit` 的整段历史下载。

## 读机器人主人的私聊

只有机器人主人本人 @ 机器人，并在消息里点名要看和某人的对话时才能读：

```
{{PYTHON}} {{SKILL_DIR}}/scripts/p2p_context.py --with <姓名> --download-dir reports/res
```

- 不需要也不能传触发消息：脚本只用客户端写进运行时配置的 `trigger_message_id`，要求客户端判定的 `trigger_is_owner` 为 true，并用 lark-cli 读回该消息核对发送人是机器人主人（`owner_open_id`），缺字段或不是主人都拒绝。
- 默认读触发消息之前 7 天、最多 50 条；对话更早就加 `--days`（最多 30）、`--limit`（最多 200）。
- 输出末尾的 `[附件]` 是下载下来的截图 / 文件，报错截图直接用读图工具看。
- 脚本输出 `[拒绝]` 就不要换参数重试，把拒绝原因写进最终报告的「需要补充的信息」。只能读主人在触发消息里点名的人，不能读别人的私聊。
- 合并转发进群的聊天记录在群聊上下文里能直接看到，不用走这一步。
- 私聊内容只拿来定位 bug：报告和文档里只写和 bug 有关的现象、报错、操作步骤，不转述其他内容，不整段引用原文。
