"""拉取群聊上下文（用户身份），按时间正序输出精简文本。

  python chat_context.py --chat-id oc_xxx [--limit 30]       # 独立使用时必须给 --chat-id
  python chat_context.py --message-ids om_a,om_b             # 指定消息（如被引用的消息）

独立使用（用户自己的 Claude Code / Codex 会话）时可以读用户自己能看到的任意群。
但如果运行时配置带 trigger_* 字段（说明是 Beetle 机器人拉起的运行，由群成员触发），就和机器人内置版一样只能读本群：
--chat-id 必须等于运行时的 chat_id，--message-ids 逐条确认属于本群，否则拒绝。
"""
import argparse
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "_shared"))
from bp_common import lark_json, load_runtime, runtime_path  # noqa: E402

MAX_MESSAGE_IDS = 20
_MESSAGE_ID = re.compile(r"^om_[A-Za-z0-9_-]+$")


def fmt(m):
    sender = (m.get("sender") or {}).get("name") or (m.get("sender") or {}).get("id", "?")
    content = "[已撤回]" if m.get("deleted") else (m.get("content") or "").replace("\r", "")
    reply = f" ↩{m['reply_to']}" if m.get("reply_to") else ""
    return f"[{m.get('create_time', '')}] {sender} ({m.get('msg_type', '')}, {m.get('message_id', '')}{reply}):\n{content}\n"


def bot_run_chat_id():
    """运行时配置带 trigger_* 字段（机器人拉起的运行）时返回本群 chat_id，否则 None（独立使用，不限制）。"""
    if not runtime_path().exists():
        return None
    rt = load_runtime()
    if not any(k.startswith("trigger_") for k in rt):
        return None
    chat_id = rt.get("chat_id")
    if not chat_id:
        raise SystemExit("[拒绝] 这是机器人拉起的运行，但运行时配置里没有本群的 chat_id，无法确认能读哪个群")
    return chat_id


def parse_message_ids(raw):
    ids = [x.strip() for x in raw.split(",") if x.strip()]
    if not ids:
        raise SystemExit("--message-ids 为空")
    if len(ids) > MAX_MESSAGE_IDS:
        raise SystemExit(f"--message-ids 一次最多 {MAX_MESSAGE_IDS} 条")
    bad = [x for x in ids if not _MESSAGE_ID.match(x)]
    if bad:
        raise SystemExit(f"消息 ID 不合法：{'、'.join(bad)}（应为 om_ 开头）")
    return ids


def message_chat_id(message_id):
    """经飞书「获取指定消息」接口查出消息所属的会话（chat_id）；查不到返回 None。"""
    d = lark_json(["api", "GET", f"/open-apis/im/v1/messages/{message_id}", "--as", "user"])
    items = (d.get("data") or {}).get("items") or []
    return items[0].get("chat_id") if items else None


def check_messages_in_chat(ids, own_chat_id, chat_of=None):
    chat_of = chat_of or message_chat_id
    for mid in ids:
        if chat_of(mid) != own_chat_id:
            raise SystemExit(f"[拒绝] 消息 {mid} 不在本群（或无法确认所属会话），机器人运行里只能读本群的消息")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--chat-id", help="群的 chat_id（oc_ 开头）；独立使用时必填")
    ap.add_argument("--limit", type=int, default=30)
    ap.add_argument("--message-ids")
    a = ap.parse_args()

    own = bot_run_chat_id()
    if a.message_ids:
        ids = parse_message_ids(a.message_ids)
        if own:
            check_messages_in_chat(ids, own)
        d = lark_json(["im", "+messages-mget", "--as", "user", "--message-ids", ",".join(ids)])
    else:
        if own:
            if a.chat_id and a.chat_id != own:
                raise SystemExit("[拒绝] 机器人运行里只能读本群的聊天记录（--chat-id 必须是本群）")
            chat_id = own
        else:
            chat_id = a.chat_id
        if not chat_id:
            raise SystemExit(
                "缺少群 ID：独立使用时需要 --chat-id oc_xxx，"
                "例如 python chat_context.py --chat-id oc_xxx --limit 30；"
                "只知道群名时先用 lark-cli 按群名搜出 chat_id，或改用 --message-ids om_a,om_b 读指定消息"
            )
        n = max(1, min(a.limit, 100))
        d = lark_json(["im", "+chat-messages-list", "--as", "user", "--chat-id", chat_id,
                       "--page-size", str(n), "--order", "desc"])

    data = d.get("data", {})
    msgs = data.get("messages") or data.get("items") or []
    msgs.sort(key=lambda m: (m.get("create_time", ""), m.get("message_position", "")))
    for m in msgs:
        print(fmt(m))
    print(f"--- 共 {len(msgs)} 条（按时间正序）")


if __name__ == "__main__":
    main()
