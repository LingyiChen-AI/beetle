"""建飞书项目工单（_shared/bp_meegle.py + create_work_item.py）测试：用假的 meegle CLI，不联网、不真建单。"""
import importlib.util
import io
import json
import sys
from datetime import datetime
from pathlib import Path

SKILLS = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SKILLS / "_shared"))


def _load(path, name):
    spec = importlib.util.spec_from_file_location(name, path)
    mod = importlib.util.module_from_spec(spec)
    sys.modules[name] = mod
    spec.loader.exec_module(mod)
    return mod


bm = _load(SKILLS / "_shared" / "bp_meegle.py", "bp_meegle_under_test")

CFG = {
    "project_key": "pk1",
    "simple_name": "bot",
    "space_name": "AI 客服",
    "work_item_type": "issue",
    "work_item_type_name": "缺陷",
    "fields": [
        {"field_key": "priority", "field_name": "优先级", "value": "2", "label": "P2"},
        {"field_key": "bug_classification", "field_name": "涉及端", "value": "c1", "label": "客户端"},
    ],
    "assignee": {"user_key": "u_li", "name": "李四"},
    "role": {"role_id": "r5", "role_name": "缺陷开发解决人员"},
}
REQUIRED = [
    {"field_key": "name", "field_type_key": "text"},
    {"field_key": "description", "field_type_key": "multi_text"},
    {"field_key": "priority", "field_type_key": "select"},
    {"field_key": "issue_reporter", "field_name": "报告人", "field_type_key": "multi_user"},
    {"field_key": "bug_classification", "field_name": "涉及端", "field_type_key": "select"},
]
REPORT = "## 售后单重复领取\n\n**复现步骤**\n\n1. 同时点两次领取\n"


class FakeCli:
    def __init__(self, same=None, required=REQUIRED, authed=True):
        self.calls = []
        self.same, self.required, self.authed = same, required, authed

    def __call__(self, args):
        self.calls.append(args)
        head = tuple(args[:2])
        if head == ("auth", "status"):
            return {"authenticated": self.authed, "host": "project.feishu.cn"}
        if head == ("user", "me"):
            return {"user_key": "u_me", "name_cn": "我"}
        if head == ("workitem", "query"):
            if not self.same:
                return {"data": {}, "list": None}
            return {"data": {"1": [{"moql_field_list": [{"key": "work_item_id", "value": {"long_value": self.same}}]}]}}
        if head == ("workitem", "meta-create-fields"):
            return {"FieldConfList": self.required}
        if head == ("workitem", "meta-roles"):
            return {"list": [{"role_id": "reporter"}, {"role_id": "r5"}]}
        if head == ("workitem", "create"):
            return {"work_item_id": 7001}
        if head == ("comment", "add"):
            return {}
        raise AssertionError(f"意外调用 {args}")

    def created_fields(self):
        args = next(a for a in self.calls if a[:2] == ["workitem", "create"])
        return {f["field_key"]: f["field_value"] for f in json.loads(args[args.index("--params") + 1])["fields"]}


def run_main(tmp_path, cli, project=None, extra=()):
    rep = tmp_path / "r.md"
    rep.write_text(REPORT, encoding="utf-8")
    proj = project if project is not None else {"record": {"meegle": True}, "meegle": CFG}
    out, err = io.StringIO(), io.StringIO()
    code = bm.main(["--project", "demo", "--file", str(rep), *extra], load_runtime=lambda: {"projects": {"demo": proj}},
                   client=bm.Client(run=cli), out=out, err=err, now=datetime(2026, 11, 20, 9, 30))
    return code, out.getvalue(), err.getvalue()


def test_creates_work_item_with_config_fields_reporter_and_assignee(tmp_path):
    cli = FakeCli()
    code, out, err = run_main(tmp_path, cli)
    assert code == 0, err
    assert out.strip() == "已创建飞书项目工单：https://project.feishu.cn/bot/issue/detail/7001"
    f = cli.created_fields()
    assert f["name"] == "售后单重复领取"
    assert f["description"].startswith("**复现步骤**")
    assert (f["priority"], f["bug_classification"]) == ("2", "c1")
    assert json.loads(f["issue_reporter"]) == ["u_me"]
    owners = json.loads(f["role_owners"])
    assert {"role": "r5", "owners": ["u_li"]} in owners and {"role": "reporter", "owners": ["u_me"]} in owners
    # 查重用的是同空间同类型、按名称精确匹配
    q = next(a for a in cli.calls if a[:2] == ["workitem", "query"])
    assert "`pk1`.`issue`" in q[-1] and "'售后单重复领取'" in q[-1]


def test_same_name_appends_comment(tmp_path):
    cli = FakeCli(same=6001)
    code, out, _ = run_main(tmp_path, cli)
    assert code == 0
    assert out.strip() == "已在飞书项目工单 https://project.feishu.cn/bot/issue/detail/6001 追加评论"
    c = next(a for a in cli.calls if a[:2] == ["comment", "add"])
    assert c[c.index("--work-item-id") + 1] == "6001"
    assert "再次复现：2026-11-20 09:30" in c[c.index("--content") + 1]
    assert not any(a[:2] == ["workitem", "create"] for a in cli.calls)


def test_missing_required_field_is_config_error(tmp_path):
    cli = FakeCli(required=REQUIRED + [{"field_key": "field_x", "field_name": "平台", "field_type_key": "select"}])
    code, _, err = run_main(tmp_path, cli)
    assert code == 2
    assert "平台" in err and "项目」页" in err
    assert not any(a[:2] == ["workitem", "create"] for a in cli.calls)


def test_not_logged_in_and_not_configured(tmp_path):
    code, _, err = run_main(tmp_path, FakeCli(authed=False))
    assert code == 3 and "去登录" in err
    code, _, err = run_main(tmp_path, FakeCli(), project={"record": {"meegle": True}})
    assert code == 2 and "还没配置飞书项目空间" in err


def test_titles_with_quotes_skip_dedupe(tmp_path):
    c = bm.Client(run=FakeCli(same=1))
    assert c.find_same_name("pk", "issue", "it's broken") is None


def test_plan_describes_config():
    s = bm.describe_plan("demo", {"record": {"meegle": True}, "meegle": CFG})
    assert "AI 客服" in s and "缺陷" in s and "李四" in s and "涉及端=客户端" in s
    assert "没有勾选" in bm.describe_plan("demo", {"record": {}})


def test_unauthed_cli_error_maps_to_exit_3(monkeypatch):
    class P:
        returncode = 1
        stdout = b""
        stderr = "Error: not logged in. Run meegle auth login".encode()

    monkeypatch.setattr(bm, "meegle_cmd", lambda: ["meegle"])
    monkeypatch.setattr(bm.subprocess, "run", lambda *a, **k: P())
    try:
        bm.run_cli(["user", "me"])
    except bm.MeegleError as e:
        assert e.code == 3
    else:
        raise AssertionError("应该报未登录")


AI_CFG = {**CFG, "fields": [{"field_key": "template", "value": "t1", "label": "用户反馈缺陷"}],
          "ai_fields": [
              {"field_key": "priority", "field_name": "优先级", "field_type": "select",
               "options": [{"id": "0", "name": "P0"}, {"id": "1", "name": "P1"}, {"id": "2", "name": "P2"}], "fallback": "2"},
              {"field_key": "bug_classification", "field_name": "涉及端", "field_type": "select",
               "options": [{"id": "c1", "name": "客户端"}, {"id": "s1", "name": "服务端"}]},
              {"field_key": "business", "field_name": "业务线", "field_type": "multi_select",
               "options": [{"id": "b1", "name": "质检"}]},
          ]}


def test_ai_fields_map_option_names_to_ids(tmp_path):
    cli = FakeCli()
    proj = {"record": {"meegle": True}, "meegle": AI_CFG}
    code, _, err = run_main(tmp_path, cli, project=proj, extra=["--field", "涉及端=服务端", "--field", "business=质检", "--field", "优先级=p1"])
    assert code == 0, err
    f = cli.created_fields()
    assert (f["priority"], f["bug_classification"], f["template"]) == ("1", "s1", "t1")
    assert json.loads(f["business"]) == ["b1"]


def test_ai_fields_fallback_and_errors(tmp_path):
    proj = {"record": {"meegle": True}, "meegle": AI_CFG}
    # 优先级没传 → 用空间默认值 P2
    cli = FakeCli()
    code, _, err = run_main(tmp_path, cli, project=proj, extra=["--field", "涉及端=客户端", "--field", "业务线=质检"])
    assert code == 0, err
    assert cli.created_fields()["priority"] == "2"
    # 涉及端没传又没有默认值；业务线写了不存在的选项；多传了不认识的字段 → 一次说清楚，不建单
    cli = FakeCli()
    code, _, err = run_main(tmp_path, cli, project=proj, extra=["--field", "业务线=售后", "--field", "平台=淘宝"])
    assert code == 2
    assert "涉及端 没有给值（可选：客户端/服务端）" in err and "业务线=售后 不是可选项" in err and "平台" in err
    assert not any(a[:2] == ["workitem", "create"] for a in cli.calls)
    code, _, err = run_main(tmp_path, FakeCli(), project=proj, extra=["--field", "坏格式"])
    assert code == 2 and "字段名=选项名" in err


def test_plan_lists_fields_to_judge():
    s = bm.describe_plan("demo", {"record": {"meegle": True}, "meegle": AI_CFG})
    assert "--field 优先级=<P0/P1/P2>" in s and "--field 涉及端=<客户端/服务端>" in s


ASSIGN_CFG = {**CFG, "assignee": {"user_key": "u_li", "name": "李四"},
              "assignees": [{"user_key": "u_li", "name": "李四"}, {"user_key": "u_zh", "name": "张三"}],
              "assign_rule": "前端问题给张三，其它给李四"}


def owners_of(cli):
    return json.loads(cli.created_fields()["role_owners"])


def test_assignee_from_candidates(tmp_path):
    proj = {"record": {"meegle": True}, "meegle": ASSIGN_CFG}
    cli = FakeCli()
    code, out, err = run_main(tmp_path, cli, project=proj, extra=["--assignee", "张三"])
    assert code == 0, err
    assert {"role": "r5", "owners": ["u_zh"]} in owners_of(cli)
    assert "[注意]" not in out
    # 不传 → 默认处理人
    cli = FakeCli()
    run_main(tmp_path, cli, project=proj)
    assert {"role": "r5", "owners": ["u_li"]} in owners_of(cli)
    # 名单外 → 改派默认并注明
    cli = FakeCli()
    code, out, _ = run_main(tmp_path, cli, project=proj, extra=["--assignee", "王五"])
    assert code == 0
    assert {"role": "r5", "owners": ["u_li"]} in owners_of(cli)
    assert "「王五」不在候选名单（李四、张三），已改派给默认处理人 李四" in out


def test_plan_shows_candidates_and_rules():
    s = bm.describe_plan("demo", {"record": {"meegle": True}, "meegle": {**ASSIGN_CFG, "priority_rule": "P0：全挂"}})
    assert "李四（默认）、张三" in s and "分派规则：前端问题给张三" in s and "优先级规则：P0：全挂" in s
