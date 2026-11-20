"""内置技能脚本测试：运行时配置读取、项目登记、文档追加、私聊放行规则。不碰真实飞书。"""
import json
import sys
from pathlib import Path

import pytest

SKILLS = Path(__file__).resolve().parent.parent
for sub in ("_shared", "chat-context/scripts", "code-sync/scripts", "feishu-doc-record/scripts"):
    sys.path.insert(0, str(SKILLS / sub))

import append_bug_doc  # noqa: E402
import bp_common  # noqa: E402
import chat_context  # noqa: E402
import p2p_context  # noqa: E402
import sync_code  # noqa: E402


def write_runtime(tmp_path, monkeypatch, data):
    p = tmp_path / "beetle.runtime.json"
    p.write_text(json.dumps(data, ensure_ascii=False), encoding="utf-8")
    monkeypatch.delenv("BUG_PATROL_RUNTIME", raising=False)
    monkeypatch.setenv("BEETLE_RUNTIME", str(p))
    return p


def test_load_runtime_reads_env_path(tmp_path, monkeypatch):
    write_runtime(tmp_path, monkeypatch, {"chat_id": "oc_x"})
    assert bp_common.load_runtime()["chat_id"] == "oc_x"


def test_load_runtime_accepts_legacy_env_name(tmp_path, monkeypatch):
    """改名过渡：只设旧名 BUG_PATROL_RUNTIME 也能读到。"""
    p = tmp_path / "bug-patrol.runtime.json"
    p.write_text(json.dumps({"chat_id": "oc_legacy"}), encoding="utf-8")
    monkeypatch.delenv("BEETLE_RUNTIME", raising=False)
    monkeypatch.setenv("BUG_PATROL_RUNTIME", str(p))
    assert bp_common.load_runtime()["chat_id"] == "oc_legacy"


def test_load_runtime_missing_env_exits_with_chinese_message(monkeypatch):
    monkeypatch.delenv("BEETLE_RUNTIME", raising=False)
    monkeypatch.delenv("BUG_PATROL_RUNTIME", raising=False)
    with pytest.raises(SystemExit, match="BEETLE_RUNTIME"):
        bp_common.load_runtime()


def test_load_runtime_missing_file_exits(tmp_path, monkeypatch):
    monkeypatch.delenv("BUG_PATROL_RUNTIME", raising=False)
    monkeypatch.setenv("BEETLE_RUNTIME", str(tmp_path / "nope.json"))
    with pytest.raises(SystemExit, match="运行时配置"):
        bp_common.load_runtime()


def test_sync_code_rejects_unknown_project(tmp_path, monkeypatch):
    write_runtime(tmp_path, monkeypatch, {"code_root": str(tmp_path / "code"), "projects": {"a": {"repo": "r"}}})
    with pytest.raises(SystemExit, match="未登记的项目"):
        sync_code.sync("not-registered")


def test_sync_code_picks_highest_version_branch(monkeypatch):
    refs = "main release/V3.2 release/V3.10 release/V3.3-20260824 release/V3.3".split()
    monkeypatch.setattr(sync_code, "git", lambda *a, **k: " ".join(refs))
    assert sync_code.latest_branch("x", "release/*") == "release/V3.10"
    assert sync_code.version_key("release/V3.3") == (3, 3, 0, 0)
    assert sync_code.version_key("release/V3.0-20260824") == (3, 0, 0, 20260824)
    assert sync_code.version_key("release/hotfix") == (-1, -1, -1, -1)


# ---------- 审查 B5 / S5：并发同步互不干扰、git 非交互 ----------

def _git(*args, cwd):
    import subprocess
    subprocess.run(["git", "-c", "user.name=t", "-c", "user.email=t@example.com", *args], cwd=cwd, check=True,
                   capture_output=True)


def make_origin(tmp_path):
    """本地仓库，两个 release 分支内容不同。"""
    src = tmp_path / "origin"
    src.mkdir()
    _git("init", "-q", "-b", "main", cwd=src)
    (src / "v.txt").write_text("base", encoding="utf-8")
    _git("add", ".", cwd=src)
    _git("commit", "-q", "-m", "base", cwd=src)
    for ver in ("V3.2", "V3.3"):
        _git("checkout", "-q", "-b", f"release/{ver}", "main", cwd=src)
        (src / "v.txt").write_text(ver, encoding="utf-8")
        _git("commit", "-q", "-am", ver, cwd=src)
    _git("checkout", "-q", "main", cwd=src)
    return src


def test_sync_code_uses_separate_worktree_per_branch(tmp_path, monkeypatch):
    origin = make_origin(tmp_path)
    code = tmp_path / "code"
    write_runtime(tmp_path, monkeypatch, {"code_root": str(code), "projects": {"a": {"repo": str(origin), "branch_glob": "release/*"}}})
    latest, branch, _ = sync_code.sync("a")
    assert branch == "release/V3.3"
    assert latest == code / "a@release_V3.3"
    assert (latest / "v.txt").read_text(encoding="utf-8") == "V3.3"
    # 另一个群切到旧分支：得到另一个目录，不把正在看 V3.3 的目录切走
    older, branch2, _ = sync_code.sync("a", "release/V3.2")
    assert branch2 == "release/V3.2" and older == code / "a@release_V3.2"
    assert (older / "v.txt").read_text(encoding="utf-8") == "V3.2"
    assert (latest / "v.txt").read_text(encoding="utf-8") == "V3.3"
    # 再同步一次（已有 worktree）：丢弃本地改动
    (latest / "v.txt").write_text("被改了", encoding="utf-8")
    sync_code.sync("a")
    assert (latest / "v.txt").read_text(encoding="utf-8") == "V3.3"
    with pytest.raises(SystemExit, match="远端没有分支"):
        sync_code.sync("a", "release/V9.9")


def test_sync_code_git_is_non_interactive_with_timeout(monkeypatch):
    seen = {}

    class R:
        returncode = 0
        stdout = "ok"
        stderr = ""

    def fake_run(cmd, **kw):
        seen.update(kw)
        return R()

    monkeypatch.setattr(sync_code.subprocess, "run", fake_run)
    assert sync_code.git(["status"], cwd=".") == "ok"
    assert seen["env"]["GIT_TERMINAL_PROMPT"] == "0"
    assert seen["env"]["GCM_INTERACTIVE"] == "never"
    assert "BatchMode=yes" in seen["env"]["GIT_SSH_COMMAND"]
    assert seen["timeout"] == sync_code.GIT_TIMEOUT
    assert seen["stdin"] == sync_code.subprocess.DEVNULL


def test_sync_code_git_timeout_exits_with_chinese_message(monkeypatch):
    def boom(cmd, **kw):
        raise sync_code.subprocess.TimeoutExpired(cmd, kw.get("timeout"))

    monkeypatch.setattr(sync_code.subprocess, "run", boom)
    with pytest.raises(SystemExit, match="超过 .* 秒没有完成"):
        sync_code.git(["fetch", "origin"], cwd=".")


def test_project_lock_is_exclusive(tmp_path):
    with sync_code.ProjectLock(tmp_path, "a"):
        with pytest.raises(SystemExit, match="正在被别的运行同步"):
            with sync_code.ProjectLock(tmp_path, "a", timeout=0):
                pass
        # 别的项目不受影响
        with sync_code.ProjectLock(tmp_path, "b", timeout=0):
            pass
    # 释放后可以再拿到
    with sync_code.ProjectLock(tmp_path, "a", timeout=0):
        pass


def test_append_doc_uses_runtime_doc_url(tmp_path, monkeypatch, capsys):
    write_runtime(tmp_path, monkeypatch, {"chat_id": "oc_x", "doc_url": "https://doc.example/docx/AAA"})
    md = tmp_path / "bug.md"
    md.write_text("## 标题甲\n\n**复现步骤**\n\n1. a\n\n**修复建议**\n\n1. b\n\n**提出人**：张三\n\n**提出时间**：2026-11-02 09:46\n",
                  encoding="utf-8")
    calls = []

    def fake(args, cwd=None):
        calls.append(args)
        return {"data": {"document": {"content": "xx 标题甲 yy"}}}

    monkeypatch.setattr(append_bug_doc, "lark_json", fake)
    monkeypatch.setattr(sys, "argv", ["append_bug_doc.py", str(md)])
    append_bug_doc.main()
    assert calls[0][calls[0].index("--doc") + 1] == "https://doc.example/docx/AAA"
    assert "append" in calls[0]
    assert "已写入并回读确认" in capsys.readouterr().out


def test_append_doc_without_doc_url_exits(tmp_path, monkeypatch):
    write_runtime(tmp_path, monkeypatch, {"chat_id": "oc_x"})
    md = tmp_path / "bug.md"
    md.write_text("## t\n", encoding="utf-8")
    monkeypatch.setattr(sys, "argv", ["append_bug_doc.py", str(md)])
    with pytest.raises(SystemExit, match="doc_url"):
        append_bug_doc.main()


def test_append_doc_rejects_wrong_field_order(tmp_path, monkeypatch):
    write_runtime(tmp_path, monkeypatch, {"doc_url": "https://d"})
    md = tmp_path / "bug.md"
    md.write_text("## t\n\n**修复建议**\n\n**复现步骤**\n\n**提出人**：a\n\n**提出时间**：b\n", encoding="utf-8")
    monkeypatch.setattr(sys, "argv", ["append_bug_doc.py", str(md)])
    with pytest.raises(SystemExit, match="顺序"):
        append_bug_doc.main()


TRIGGER_MSG = {"data": {"messages": [{"sender": {"id": "ou_owner"}, "content": "@机器人 看下我和李四的对话",
                                       "create_time": "2026-11-02 09:46"}]}}


# trigger_sender_open_id 是机器人应用命名空间的 id，故意和 owner_open_id（lark-cli 命名空间）不同
RT = {"owner_open_id": "ou_owner", "trigger_message_id": "om_1", "trigger_sender_open_id": "ou_bot_ns",
      "trigger_is_owner": True}


def test_p2p_has_no_trigger_argument():
    import inspect
    assert "--trigger" not in inspect.getsource(p2p_context.main)


def test_old_owner_message_cannot_be_used(tmp_path, monkeypatch):
    """运行时的触发人不是主人时拒绝；agent 没有参数可以改用主人的旧消息，也不会去读别的消息。"""
    write_runtime(tmp_path, monkeypatch, {**RT, "trigger_message_id": "om_other", "trigger_is_owner": False})
    calls = []
    monkeypatch.setattr(p2p_context, "lark_json", lambda *a, **k: calls.append(a) or TRIGGER_MSG)
    with pytest.raises(SystemExit, match="拒绝"):
        p2p_context.check_trigger("李四")
    assert calls == []


def test_p2p_refuses_when_fetched_sender_is_not_owner(tmp_path, monkeypatch):
    write_runtime(tmp_path, monkeypatch, RT)
    other = {"data": {"messages": [{**TRIGGER_MSG["data"]["messages"][0], "sender": {"id": "ou_other"}}]}}
    monkeypatch.setattr(p2p_context, "lark_json", lambda *a, **k: other)
    with pytest.raises(SystemExit, match="拒绝"):
        p2p_context.check_trigger("李四")


@pytest.mark.parametrize("bad", [None, "true", 1])
def test_trigger_is_owner_must_be_strict_true(tmp_path, monkeypatch, bad):
    rt = {**RT, "trigger_is_owner": bad} if bad is not None else {k: v for k, v in RT.items() if k != "trigger_is_owner"}
    write_runtime(tmp_path, monkeypatch, rt)
    calls = []
    monkeypatch.setattr(p2p_context, "lark_json", lambda *a, **k: calls.append(a) or TRIGGER_MSG)
    with pytest.raises(SystemExit, match="拒绝"):
        p2p_context.check_trigger("李四")
    assert calls == []


@pytest.mark.parametrize("missing", ["owner_open_id", "trigger_message_id", "trigger_is_owner"])
def test_missing_trigger_fields_refused(tmp_path, monkeypatch, missing):
    write_runtime(tmp_path, monkeypatch, {k: v for k, v in RT.items() if k != missing})
    monkeypatch.setattr(p2p_context, "lark_json", lambda *a, **k: TRIGGER_MSG)
    with pytest.raises(SystemExit, match="拒绝"):
        p2p_context.check_trigger("李四")


def test_p2p_requires_peer_name_in_trigger(tmp_path, monkeypatch):
    write_runtime(tmp_path, monkeypatch, RT)
    monkeypatch.setattr(p2p_context, "lark_json", lambda *a, **k: TRIGGER_MSG)
    with pytest.raises(SystemExit, match="点名"):
        p2p_context.check_trigger("王五")


def test_p2p_allows_owner_trigger_from_runtime_only(tmp_path, monkeypatch):
    write_runtime(tmp_path, monkeypatch, RT)
    seen = []
    monkeypatch.setattr(p2p_context, "lark_json", lambda args, **k: seen.append(args) or TRIGGER_MSG)
    assert p2p_context.check_trigger("李四")["create_time"] == "2026-11-02 09:46"
    assert "om_1" in seen[0]


# ---------- 审查 B4：chat_context 只能读本群 ----------

def run_chat_context(monkeypatch, argv, replies):
    """以假 lark-cli 运行 chat_context.main()，返回调过的 lark-cli 参数列表。replies: 参数 → 返回值。"""
    calls = []

    def fake(args, cwd=None):
        calls.append(args)
        for key, value in replies.items():
            if key in args:
                return value
        return {"data": {"messages": []}}

    monkeypatch.setattr(chat_context, "lark_json", fake)
    monkeypatch.setattr(sys, "argv", ["chat_context.py", *argv])
    chat_context.main()
    return calls


def test_chat_context_rejects_other_chat_id_before_reading(tmp_path, monkeypatch):
    write_runtime(tmp_path, monkeypatch, {"chat_id": "oc_own"})
    with pytest.raises(SystemExit, match="拒绝"):
        run_chat_context(monkeypatch, ["--chat-id", "oc_p2p_of_owner"], {})
    # 本群 / 不写 --chat-id 照常读本群
    calls = run_chat_context(monkeypatch, ["--chat-id", "oc_own"], {})
    assert calls[0][calls[0].index("--chat-id") + 1] == "oc_own"
    calls = run_chat_context(monkeypatch, [], {})
    assert calls[0][calls[0].index("--chat-id") + 1] == "oc_own"


def test_chat_context_without_runtime_chat_id_refuses(tmp_path, monkeypatch):
    write_runtime(tmp_path, monkeypatch, {"owner_open_id": "ou_owner"})
    with pytest.raises(SystemExit, match="拒绝"):
        run_chat_context(monkeypatch, [], {})


def test_chat_context_message_ids_must_all_be_in_own_chat(tmp_path, monkeypatch):
    write_runtime(tmp_path, monkeypatch, {"chat_id": "oc_own"})
    owners = {"om_a": "oc_own", "om_b": "oc_other"}
    monkeypatch.setattr(chat_context, "message_chat_id", lambda mid: owners.get(mid))
    with pytest.raises(SystemExit, match="不在本群"):
        run_chat_context(monkeypatch, ["--message-ids", "om_a,om_b"], {})
    with pytest.raises(SystemExit, match="不在本群"):
        run_chat_context(monkeypatch, ["--message-ids", "om_unknown"], {})
    calls = run_chat_context(monkeypatch, ["--message-ids", "om_a"], {"+messages-mget": {"data": {"messages": []}}})
    assert calls == [["im", "+messages-mget", "--as", "user", "--message-ids", "om_a"]]


def test_chat_context_message_chat_id_reads_get_message_api(monkeypatch):
    seen = []
    monkeypatch.setattr(chat_context, "lark_json",
                        lambda args, cwd=None: seen.append(args) or {"data": {"items": [{"chat_id": "oc_x"}]}})
    assert chat_context.message_chat_id("om_1") == "oc_x"
    assert seen[0][:3] == ["api", "GET", "/open-apis/im/v1/messages/om_1"]
    monkeypatch.setattr(chat_context, "lark_json", lambda args, cwd=None: {"data": {"items": []}})
    assert chat_context.message_chat_id("om_1") is None


@pytest.mark.parametrize("raw", ["om_a,../x", "om_a/b", "oc_x", ",".join(f"om_{i}" for i in range(21))])
def test_chat_context_rejects_bad_message_ids(raw):
    with pytest.raises(SystemExit):
        chat_context.parse_message_ids(raw)
