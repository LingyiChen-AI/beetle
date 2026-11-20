"""技能中心独立版（catalog/bug-hunt）脚本测试。

独立版和机器人内置版的模块同名（bp_common / chat_context / sync_code），同一个 pytest 进程里会互相覆盖，
所以这里按文件路径加载成独立模块名，并在加载期间临时换掉 sys.modules 里的 bp_common。
"""
import importlib.util
import json
import subprocess
import sys
from pathlib import Path

import pytest

CATALOG = Path(__file__).resolve().parents[2] / "catalog" / "bug-hunt"


def load_catalog(monkeypatch, name):
    shared = CATALOG / "_shared"
    monkeypatch.syspath_prepend(str(shared))
    monkeypatch.delitem(sys.modules, "bp_common", raising=False)
    spec = importlib.util.spec_from_file_location(f"catalog_{name}", CATALOG / "scripts" / f"{name}.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def write_runtime(tmp_path, monkeypatch, data):
    p = tmp_path / "rt.json"
    p.write_text(json.dumps(data, ensure_ascii=False), encoding="utf-8")
    monkeypatch.delenv("BUG_PATROL_RUNTIME", raising=False)
    monkeypatch.setenv("BEETLE_RUNTIME", str(p))
    return p


# ---------- 审查 B4：独立版只有在机器人运行（运行时带 trigger_* 字段）里才限制本群 ----------

def test_catalog_chat_context_free_when_runtime_has_no_trigger(tmp_path, monkeypatch):
    cc = load_catalog(monkeypatch, "chat_context")
    write_runtime(tmp_path, monkeypatch, {"owner_open_id": "ou_o", "projects": {}})
    assert cc.bot_run_chat_id() is None
    calls = []
    monkeypatch.setattr(cc, "lark_json", lambda args, cwd=None: calls.append(args) or {"data": {"messages": []}})
    monkeypatch.setattr(sys, "argv", ["chat_context.py", "--chat-id", "oc_any"])
    cc.main()
    assert calls[0][calls[0].index("--chat-id") + 1] == "oc_any"
    # 指定消息也不查所属会话
    calls.clear()
    monkeypatch.setattr(sys, "argv", ["chat_context.py", "--message-ids", "om_a"])
    cc.main()
    assert calls == [["im", "+messages-mget", "--as", "user", "--message-ids", "om_a"]]


def test_catalog_chat_context_restricted_inside_bot_run(tmp_path, monkeypatch):
    cc = load_catalog(monkeypatch, "chat_context")
    write_runtime(tmp_path, monkeypatch, {"chat_id": "oc_own", "trigger_message_id": "om_t", "trigger_is_owner": False})
    assert cc.bot_run_chat_id() == "oc_own"
    calls = []
    monkeypatch.setattr(cc, "lark_json", lambda args, cwd=None: calls.append(args) or {"data": {"messages": []}})
    monkeypatch.setattr(sys, "argv", ["chat_context.py", "--chat-id", "oc_other"])
    with pytest.raises(SystemExit, match="拒绝"):
        cc.main()
    assert calls == []
    monkeypatch.setattr(cc, "message_chat_id", lambda mid: "oc_other")
    monkeypatch.setattr(sys, "argv", ["chat_context.py", "--message-ids", "om_a"])
    with pytest.raises(SystemExit, match="不在本群"):
        cc.main()
    assert calls == []


def test_catalog_chat_context_without_runtime_needs_chat_id():
    """没有环境变量、技能目录也没有 runtime.json：独立使用，缺 --chat-id 时给中文用法提示（不是拒绝）。"""
    env = {k: v for k, v in __import__("os").environ.items() if k not in ("BEETLE_RUNTIME", "BUG_PATROL_RUNTIME")}
    assert not (CATALOG / "beetle.runtime.json").exists() and not (CATALOG / "bug-patrol.runtime.json").exists()
    r = subprocess.run([sys.executable, "-I", str(CATALOG / "scripts" / "chat_context.py")],
                       capture_output=True, encoding="utf-8", env=env)
    assert r.returncode != 0
    assert "缺少群 ID" in (r.stdout + r.stderr)
    assert "拒绝" not in (r.stdout + r.stderr)


# ---------- 数据源：项目的 dbs 是对象数组（旧版是别名字符串），--list 两种都认 ----------

@pytest.mark.parametrize("script_dir", ["bug-hunt", "code-review"])
def test_catalog_sync_code_list_shows_datasources(tmp_path, monkeypatch, capsys, script_dir):
    shared = CATALOG.parent / script_dir / "_shared"
    monkeypatch.syspath_prepend(str(shared))
    monkeypatch.delitem(sys.modules, "bp_common", raising=False)
    spec = importlib.util.spec_from_file_location(f"catalog_sync_{script_dir.replace('-', '_')}", CATALOG.parent / script_dir / "scripts" / "sync_code.py")
    sc = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(sc)
    write_runtime(tmp_path, monkeypatch, {"projects": {
        "svc": {"description": "演示", "dbs": [
            {"alias": "demo_main", "kind": "mysql", "description": "", "access": "direct", "database": "demo_main"},
            {"alias": "demo_log", "kind": "mysql", "description": "", "access": "skill", "skill": "my-sql", "database": "demo_log", "note": "只读"},
        ]},
        "old": {"description": "旧格式", "dbs": ["demo_a"]},
        "none": {"description": "没有库"},
    }})
    monkeypatch.setattr(sys, "argv", ["sync_code.py", "--list"])
    sc.main()
    out = capsys.readouterr().out
    assert "demo_main（mysql，直连，用 db-query 查）" in out
    assert "demo_log（mysql，用技能 my-sql 查，库 demo_log，只读）" in out
    assert "数据源=demo_a" in out and "数据源=无" in out
