"""拉取本群的群聊上下文（用户身份），按时间正序输出精简文本。

  python chat_context.py [--limit 30]                        # 读运行时配置里本群（chat_id）的最近消息
  python chat_context.py --message-ids om_a,om_b             # 指定本群的消息（如被引用的消息）

只能读本群：脚本以机器人主人的用户身份调 lark-cli，能看到主人所有的群和私聊，所以不信任调用方：
  - --chat-id 可以不写；写了必须等于运行时配置里的 chat_id（本群），否则拒绝。
  - --message-ids 先逐条查出所属会话，任何一条不在本群就整批拒绝，不输出内容。
读主人与某人的私聊只能用带门禁的 p2p_context.py。
"""
import argparse
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "_shared"))
from bp_common import lark_json, load_runtime  # noqa: E402

MAX_MESSAGE_IDS = 20
_MESSAGE_ID = re.compile(r"^om_[A-Za-z0-9_-]+$")


def fmt(m):
    sender = (m.get("sender") or {}).get("name") or (m.get("sender") or {}).get("id", "?")
    content = "[已撤回]" if m.get("deleted") else (m.get("content") or "").replace("\r", "")
    reply = f" ↩{m['reply_to']}" if m.get("reply_to") else ""
    return f"[{m.get('create_time', '')}] {sender} ({m.get('msg_type', '')}, {m.get('message_id', '')}{reply}):\n{content}\n"


def with_download(args, download_dir):
    """要下载附件时加 --download-resources，并在下载目录里运行（lark-cli 下到 ./lark-im-resources/）。"""
    if not download_dir:
        return args, None
    Path(download_dir).mkdir(parents=True, exist_ok=True)
    return args + ["--download-resources"], download_dir


def print_attachments(download_dir):
    """列出下载下来的图片 / 文件（绝对路径），供读图工具直接打开。"""
    if not download_dir:
        return
    res = Path(download_dir) / "lark-im-resources"
    files = sorted(p for p in res.rglob("*") if p.is_file()) if res.exists() else []
    for p in files:
        print(f"[附件] {p.resolve().as_posix()}")
    if not files:
        print("--- 没有可下载的图片 / 文件")


def runtime_chat_id():
    chat_id = load_runtime().get("chat_id")
    if not chat_id:
        raise SystemExit("[拒绝] 运行时配置里没有本群的 chat_id，无法确认能读哪个群")
    return chat_id


def check_chat_arg(arg_chat_id, own_chat_id):
    """--chat-id 只能是本群。"""
    if arg_chat_id and arg_chat_id != own_chat_id:
        raise SystemExit("[拒绝] 只能读本群的聊天记录（--chat-id 必须是本群）。读主人与某人的私聊请用 p2p_context.py")
    return own_chat_id


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
    """每条消息都必须属于本群；有一条不是（或查不到所属会话）就整批拒绝。"""
    chat_of = chat_of or message_chat_id
    for mid in ids:
        if chat_of(mid) != own_chat_id:
            raise SystemExit(f"[拒绝] 消息 {mid} 不在本群（或无法确认所属会话），只能读本群的消息")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--chat-id", help="可不写；写了必须是本群的 chat_id")
    ap.add_argument("--limit", type=int, default=30)
    ap.add_argument("--message-ids")
    ap.add_argument("--download-dir", help="把消息里的图片 / 文件下载到这个目录（截图里常有报错和页面现象）")
    a = ap.parse_args()

    own = runtime_chat_id()
    if a.message_ids:
        ids = parse_message_ids(a.message_ids)
        check_messages_in_chat(ids, own)
        args = ["im", "+messages-mget", "--as", "user", "--message-ids", ",".join(ids)]
    else:
        chat_id = check_chat_arg(a.chat_id, own)
        n = max(1, min(a.limit, 100))
        args = ["im", "+chat-messages-list", "--as", "user", "--chat-id", chat_id,
                "--page-size", str(n), "--order", "desc"]
    d = lark_json(*with_download(args, a.download_dir))

    data = d.get("data", {})
    msgs = data.get("messages") or data.get("items") or []
    msgs.sort(key=lambda m: (m.get("create_time", ""), m.get("message_position", "")))
    for m in msgs:
        print(fmt(m))
    print(f"--- 共 {len(msgs)} 条（按时间正序）")
    print_attachments(a.download_dir)


if __name__ == "__main__":
    main()
