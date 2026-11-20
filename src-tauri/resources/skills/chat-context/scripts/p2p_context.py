"""读机器人主人和某个人的私聊记录（主人用户身份），按时间正序输出精简文本。

  python p2p_context.py --with 李四 [--days 7] [--limit 50] [--download-dir reports/res]

私聊是主人的隐私，脚本自己做校验，不信任调用方：
  1. 触发消息 ID 只取自运行时配置（客户端每次运行写入的 trigger_message_id），命令行不接受传入，
     agent 无法改用别的旧消息。两道核对：运行时的 trigger_is_owner 必须为 true（客户端按跨应用一致的
     union_id 判定），且脚本用 lark-cli 读回触发消息后飞书返回的发送人必须等于 owner_open_id
     （两者同属 lark-cli 应用的 open_id 命名空间；机器人事件里的 trigger_sender_open_id 属于另一个应用，不拿来比）。
     缺任何一个字段直接拒绝。
  2. 触发消息正文里必须出现 --with 的名字（主人点名要看和谁的对话）。
  3. 按名字搜人，必须唯一命中，且主人和对方聊过（有 p2p 会话）。
任何一条不满足就拒绝，不读私聊。
"""
import argparse
import sys
from datetime import datetime, timedelta, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "_shared"))
from bp_common import lark_json, load_runtime  # noqa: E402
from chat_context import fmt  # noqa: E402


def check_trigger(name):
    rt = load_runtime()
    owner_id = rt.get("owner_open_id")
    trigger = rt.get("trigger_message_id")
    if not owner_id:
        raise SystemExit("[拒绝] 运行时配置里没有 owner_open_id，无法确认触发人是机器人主人")
    if not trigger:
        raise SystemExit("[拒绝] 运行时配置里没有 trigger_message_id，无法确认本次触发消息")
    # trigger_sender_open_id 属于机器人应用的命名空间，和 owner_open_id（lark-cli 命名空间）不可比；
    # 客户端按 union_id 判定后写入 trigger_is_owner，必须严格为 true。
    if rt.get("trigger_is_owner") is not True:
        raise SystemExit("[拒绝] 只有机器人主人本人 @机器人 时才能读主人的私聊，这次触发人不是主人")
    d = lark_json(["im", "+messages-mget", "--as", "user", "--message-ids", trigger])
    msgs = (d.get("data") or {}).get("messages") or []
    if not msgs:
        raise SystemExit(f"[拒绝] 读不到触发消息 {trigger}")
    m = msgs[0]
    sender = (m.get("sender") or {}).get("id")
    if sender != owner_id:
        raise SystemExit("[拒绝] 只有机器人主人本人 @机器人 时才能读主人的私聊，这次触发人不是主人")
    if name not in (m.get("content") or ""):
        raise SystemExit(f"[拒绝] 触发消息里没有点名「{name}」，不读私聊。请主人在 @机器人 的消息里写明要看和谁的对话")
    return m


def find_peer(name):
    d = lark_json(["contact", "+search-user", "--as", "user", "--query", name])
    users = (d.get("data") or {}).get("users") or []
    exact = [u for u in users if u.get("localized_name") == name] or users
    if not exact:
        raise SystemExit(f"[拒绝] 通讯录里找不到「{name}」")
    if len(exact) > 1:
        cands = "、".join(f"{u.get('localized_name')}（{u.get('department', '')}）" for u in exact[:5])
        raise SystemExit(f"[拒绝] 「{name}」匹配到多个人：{cands}。请主人写全名")
    u = exact[0]
    if not u.get("p2p_chat_id"):
        raise SystemExit(f"[拒绝] 主人和「{name}」没有私聊记录")
    return u


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--with", dest="peer", required=True, help="对方姓名，必须出现在触发消息里")
    ap.add_argument("--days", type=int, default=7, help="往前读几天，默认 7，最多 30")
    ap.add_argument("--limit", type=int, default=50, help="最多读几条，默认 50，最多 200")
    ap.add_argument("--include-after", action="store_true", help="也读触发消息之后的私聊（默认不读）")
    ap.add_argument("--download-dir", help="把图片 / 文件下载到这个目录（截图里常有报错）")
    a = ap.parse_args()

    name = a.peer.strip().lstrip("@")
    trig = check_trigger(name)
    peer = find_peer(name)

    days = max(1, min(a.days, 30))
    limit = max(1, min(a.limit, 200))
    cst = timezone(timedelta(hours=8))
    # 默认只读到触发消息那一分钟为止：主人 @机器人 之后私聊里聊的别的事不读
    end = datetime.strptime(trig["create_time"], "%Y-%m-%d %H:%M").replace(tzinfo=cst) + timedelta(minutes=1)
    if a.include_after:
        end = datetime.now(cst)
    start = end - timedelta(days=days)
    fmt_t = "%Y-%m-%dT%H:%M:%S+08:00"
    args = ["im", "+chat-messages-list", "--as", "user", "--chat-id", peer["p2p_chat_id"],
            "--start", start.strftime(fmt_t), "--end", end.strftime(fmt_t), "--order", "desc", "--no-reactions",
            "--page-size", str(min(limit, 50))]
    if limit > 50:
        args += ["--page-all", "--page-limit", str((limit + 49) // 50)]
    cwd = None
    if a.download_dir:
        Path(a.download_dir).mkdir(parents=True, exist_ok=True)
        cwd = a.download_dir
        args.append("--download-resources")
    d = lark_json(args, cwd=cwd)

    data = d.get("data", {})
    msgs = (data.get("messages") or data.get("items") or [])[:limit]
    msgs.sort(key=lambda m: (m.get("create_time", ""), m.get("message_position", "")))
    print(f"--- 主人与 {peer.get('localized_name')}（{peer.get('department', '')}）的私聊，{start:%m-%d %H:%M} ~ {end:%m-%d %H:%M}")
    for m in msgs:
        print(fmt(m))
    print(f"--- 共 {len(msgs)} 条（按时间正序）")
    if a.download_dir:
        res = Path(a.download_dir) / "lark-im-resources"
        files = sorted(p for p in res.rglob("*") if p.is_file()) if res.exists() else []
        for p in files:
            print(f"[附件] {p.resolve().as_posix()}")


if __name__ == "__main__":
    main()
