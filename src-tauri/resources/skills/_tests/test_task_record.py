"""建飞书任务（_shared/bp_task.py）测试：假的 lark-cli，不真建任务。"""
import importlib.util
import io
import json
import sys
from datetime import datetime
from pathlib import Path

SKILLS = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SKILLS / "_shared"))
spec = importlib.util.spec_from_file_location("bp_task_under_test", SKILLS / "_shared" / "bp_task.py")
bt = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bt)

NOW = datetime(2026, 11, 20, 9, 30)


def run(tmp_path, cfg, reply=(0, json.dumps({"ok": True, "data": {"task": {"guid": "g1", "url": "https://applink.feishu.cn/t/1"}}}), "")):
    rep = tmp_path / "r.md"
    rep.write_text("## 调研：退款接口限流方案\n\n需要对比两种做法。\n", encoding="utf-8")
    calls = []

    def runner(args):
        calls.append(args)
        return reply

    out, err = io.StringIO(), io.StringIO()
    code = bt.main(["--project", "demo", "--file", str(rep)], load_runtime=lambda: {"projects": {"demo": {"feishu_task": cfg}}},
                   runner=runner, out=out, err=err, now=NOW)
    return code, out.getvalue(), err.getvalue(), calls


def test_creates_task_with_tasklist_assignee_and_due(tmp_path):
    code, out, err, calls = run(tmp_path, {"tasklist_guid": "tl1", "assignee_open_id": "ou_a", "due_days": 3})
    assert code == 0, err
    assert out.strip() == "已创建飞书任务：https://applink.feishu.cn/t/1"
    a = calls[0]
    assert a[:4] == ["task", "+create", "--as", "user"]
    assert a[a.index("--summary") + 1] == "调研：退款接口限流方案"
    assert a[a.index("--assignee") + 1] == "ou_a"
    assert a[a.index("--tasklist-id") + 1] == "tl1"
    assert a[a.index("--due") + 1] == "2026-11-23"
    # 同一天同项目同标题幂等键相同
    assert a[a.index("--idempotency-key") + 1] == bt.idempotency_key("demo", "调研：退款接口限流方案", NOW)


def test_minimal_config_and_failure(tmp_path):
    code, _, _, calls = run(tmp_path, {})
    assert code == 0
    assert "--assignee" not in calls[0] and "--tasklist-id" not in calls[0] and "--due" not in calls[0]
    code, _, err, _ = run(tmp_path, {}, reply=(1, "", "permission denied"))
    assert code == 4 and "permission denied" in err
    code, _, err, _ = run(tmp_path, {}, reply=(0, json.dumps({"ok": False, "error": {"message": "no scope"}}), ""))
    assert code == 4 and "no scope" in err
    code, _, err, _ = run(tmp_path, {}, reply=(0, json.dumps({"ok": True, "data": {"error": "failed to parse due time"}}), ""))
    assert code == 4 and "due time" in err
