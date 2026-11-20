"""建 issue（_shared/bp_issue.py + create_issue.py）测试：全部用假 HTTP，不访问网络、不读真实 token。

机器人运行时技能（skills/_shared）和技能中心 bug-hunt（catalog/bug-hunt/_shared）各带一份 bp_issue.py，要求内容完全相同；
这里按文件路径加载成独立模块名，避免和别的测试里的同名模块互相覆盖。
"""
import importlib.util
import io
import json
import ssl
import sys
import urllib.error
import urllib.parse
from datetime import datetime
from pathlib import Path

import pytest

SKILLS = Path(__file__).resolve().parent.parent
CATALOG = SKILLS.parent / "catalog" / "bug-hunt"
TOKEN = "tok_SECRET_0123456789abcdef"


def _load(path, name):
    spec = importlib.util.spec_from_file_location(name, path)
    mod = importlib.util.module_from_spec(spec)
    sys.modules[name] = mod  # dataclass 需要能在 sys.modules 里找到所在模块
    spec.loader.exec_module(mod)
    return mod


bi = _load(SKILLS / "_shared" / "bp_issue.py", "bp_issue_under_test")


@pytest.fixture(autouse=True)
def no_real_token(monkeypatch):
    """不读本机注册表 / 环境变量里真实的 token。"""
    monkeypatch.setattr(bi, "_read_user_env", lambda name: None)
    for k in ("GITEE_TOKEN", "GITHUB_TOKEN", "MY_TOKEN"):
        monkeypatch.delenv(k, raising=False)


class FakeHttp:
    """按 (方法, 路径) 返回预设结果；handler 可以是值、异常或函数。记录每次调用。"""

    def __init__(self, routes):
        self.routes = routes
        self.calls = []
        self.sleeps = []

    def sleep(self, seconds):
        self.sleeps.append(seconds)  # 不真睡，只记录

    def request(self, method, url, *, headers=None, form=None, json_body=None, retry=True):
        u = urllib.parse.urlsplit(url)
        self.calls.append({"method": method, "path": u.path, "query": dict(urllib.parse.parse_qsl(u.query)),
                           "headers": headers or {}, "form": form, "json": json_body, "retry": retry})
        h = self.routes.get((method, u.path))
        if h is None:
            raise AssertionError(f"没有预设 {method} {u.path}")
        if callable(h):
            h = h(self.calls[-1])
        if isinstance(h, Exception):
            raise h
        return 200, h

    def paths(self):
        return [(c["method"], c["path"]) for c in self.calls]


def gitee_target():
    return bi.Target("gitee", "example", "demo-issues", "GITEE_TOKEN")


def github_target():
    return bi.Target("github", "example", "demo-issues", "GITHUB_TOKEN")


# ───────────── 平台选择与仓库解析 ─────────────

@pytest.mark.parametrize("url,want", [
    ("git@gitee.com:example/demo-issues.git", ("gitee.com", "example", "demo-issues")),
    ("git@github.com:example/demo-issues", ("github.com", "example", "demo-issues")),
    ("https://gitee.com/example/demo-issues", ("gitee.com", "example", "demo-issues")),
    ("https://github.com/example/demo-issues.git/", ("github.com", "example", "demo-issues")),
    ("https://me:pass@GitHub.com/a/b.git", ("github.com", "a", "b")),
    ("ssh://git@gitee.com:22/a/b.git", ("gitee.com", "a", "b")),
    ("gitee.com/a/demo.git", ("gitee.com", "a", "demo")),
    ("https://gitee.com/onlyowner", None),
    ("", None),
])
def test_parse_repo_forms(url, want):
    assert bi.parse_repo(url) == want


def test_resolve_target_picks_platform_and_default_token_env():
    t = bi.resolve_target({"repo": "git@gitee.com:a/demo.git"})
    assert (t.platform, t.full, t.token_env, t.label) == ("gitee", "a/demo", "GITEE_TOKEN", "Gitee · a/demo")
    t = bi.resolve_target({"repo": "https://github.com/o/r.git"})
    assert (t.platform, t.full, t.token_env) == ("github", "o/r", "GITHUB_TOKEN")
    assert isinstance(bi.build_provider(t, TOKEN, FakeHttp({})), bi.GitHub)
    assert isinstance(bi.build_provider(bi.resolve_target({"repo": "gitee.com/a/b"}), TOKEN, FakeHttp({})), bi.Gitee)


def test_resolve_target_overrides_and_unsupported_host():
    t = bi.resolve_target({"repo": "git@gitee.com:a/demo.git", "issue_repo": " team/issues.git ", "token_env": "MY_TOKEN"})
    assert (t.platform, t.full, t.token_env) == ("gitee", "team/issues", "MY_TOKEN")
    with pytest.raises(bi.IssueError) as e:
        bi.resolve_target({"repo": "git@gitlab.com:a/b.git"})
    assert e.value.code == bi.EXIT_USAGE and "gitee.com / github.com" in str(e.value)
    t = bi.resolve_target({"repo": "git@gitlab.com:a/b.git", "issue_repo": "https://github.com/o/r"})
    assert (t.platform, t.full) == ("github", "o/r")


# ───────────── 去重：已有 → 评论，没有 → 新建 ─────────────

NOW = datetime(2026, 11, 19, 10, 30)


def test_github_existing_issue_found_by_list_gets_comment():
    """搜索有索引延迟搜不到，靠 bug 标签列表比对标题找到 → 追加评论，不新建。"""
    http = FakeHttp({
        ("GET", "/search/issues"): {"items": []},
        ("GET", "/repos/example/demo-issues/issues"): [
            {"number": 3, "title": "别的问题", "state": "open", "html_url": "u3"},
            {"number": 7, "title": "导出报表 500", "state": "open", "html_url": "https://github.com/x/7"},
            {"number": 8, "title": "导出报表 500", "state": "open", "pull_request": {}, "html_url": "pr"},
        ],
        ("POST", "/repos/example/demo-issues/issues/7/comments"): {"id": 1},
    })
    p = bi.GitHub(github_target(), TOKEN, http)
    action, issue = bi.record_issue(p, "导出报表 500", "正文", now=NOW)
    assert (action, issue) == ("commented", {"number": "7", "url": "https://github.com/x/7"})
    body = http.calls[-1]["json"]["body"]
    assert body.startswith("正文") and "再次复现：2026-11-19 10:30" in body
    assert ("POST", "/repos/example/demo-issues/issues") not in http.paths()
    search = http.calls[0]["query"]["q"]
    assert "repo:example/demo-issues" in search and "is:open" in search and "in:title" in search
    assert http.calls[1]["query"] == {"state": "open", "labels": "bug", "per_page": "100"}
    assert http.calls[0]["headers"]["Authorization"] == f"Bearer {TOKEN}"


def test_github_new_issue_is_created_with_bug_label():
    http = FakeHttp({
        ("GET", "/search/issues"): {"items": [{"number": 1, "title": "导出报表 500（旧）", "state": "open"}]},
        ("GET", "/repos/example/demo-issues/issues"): [],
        ("POST", "/repos/example/demo-issues/issues"): {"number": 9, "html_url": "https://github.com/x/9"},
    })
    action, issue = bi.record_issue(bi.GitHub(github_target(), TOKEN, http), "导出报表 500", "正文", now=NOW)
    assert (action, issue["url"]) == ("created", "https://github.com/x/9")
    create = http.calls[-1]
    assert create["json"] == {"title": "导出报表 500", "body": "正文", "labels": ["bug"]}
    assert create["retry"] is False, "建 issue 不能在 HTTP 层盲目重发"


def test_gitee_existing_issue_found_by_search_gets_comment():
    http = FakeHttp({
        ("GET", "/api/v5/search/issues"): [{"number": "IKWMBW", "title": "导出报表 500", "state": "open", "html_url": "https://gitee.com/x/IKWMBW"}],
        ("POST", "/api/v5/repos/example/demo-issues/issues/IKWMBW/comments"): {"id": 1},
    })
    action, issue = bi.record_issue(bi.Gitee(gitee_target(), TOKEN, http), "导出报表 500", "正文", now=NOW)
    assert (action, issue["number"]) == ("commented", "IKWMBW")
    search = http.calls[0]["query"]
    assert search["q"] == "导出报表 500" and search["repo"] == "example/demo-issues" and search["state"] == "open"
    assert http.calls[-1]["form"]["body"].endswith("再次复现：2026-11-19 10:30")
    assert http.calls[-1]["form"]["access_token"] == TOKEN


def test_gitee_new_issue_creates_label_then_issue_and_patches_label():
    http = FakeHttp({
        ("GET", "/api/v5/search/issues"): [],
        ("GET", "/api/v5/repos/example/demo-issues/issues"): [],
        ("POST", "/api/v5/repos/example/demo-issues/labels"): bi.ApiError(409, "接口返回 409：标签已存在"),
        ("POST", "/api/v5/repos/example/issues"): {"number": "IKWMBW", "html_url": "https://gitee.com/x/IKWMBW", "labels": []},
        ("GET", "/api/v5/repos/example/demo-issues/issues/IKWMBW"): {"number": "IKWMBW", "labels": []},
        ("POST", "/api/v5/repos/example/demo-issues/issues/IKWMBW/labels"): [{"name": "bug"}],
    })
    action, issue = bi.record_issue(bi.Gitee(gitee_target(), TOKEN, http), "导出报表 500", "正文", now=NOW)
    assert (action, issue) == ("created", {"number": "IKWMBW", "url": "https://gitee.com/x/IKWMBW"})
    create = next(c for c in http.calls if c["path"] == "/api/v5/repos/example/issues")
    assert create["form"]["repo"] == "demo-issues" and create["form"]["labels"] == "bug"
    assert create["form"]["title"] == "导出报表 500" and create["retry"] is False
    patch = http.calls[-1]
    assert patch["json"] == ["bug"], "补打标签要 JSON 数组体"


def test_gitee_label_409_is_success_but_other_errors_raise():
    http = FakeHttp({("POST", "/api/v5/repos/example/demo-issues/labels"): bi.ApiError(409, "已存在")})
    bi.Gitee(gitee_target(), TOKEN, http).ensure_label()  # 不抛
    http = FakeHttp({("POST", "/api/v5/repos/example/demo-issues/labels"): bi.ApiError(403, "无权限")})
    with pytest.raises(bi.ApiError):
        bi.Gitee(gitee_target(), TOKEN, http).ensure_label()


def test_gitee_close_uses_owner_only_path_with_repo_in_form():
    http = FakeHttp({("PATCH", "/api/v5/repos/example/issues/IKWMBW"): {"state": "closed"}})
    bi.Gitee(gitee_target(), TOKEN, http).close_issue("IKWMBW")
    assert http.calls[0]["form"] == {"repo": "demo-issues", "state": "closed", "access_token": TOKEN}


def test_create_network_error_rechecks_before_retrying():
    """建 issue 时 SSL EOF：服务端可能已经建好，重试前先按标题再查，查到就不再建第二条。"""
    state = {"find": 0, "create": 0}

    class P:
        def find_open_issue(self, title):
            state["find"] += 1
            return None if state["find"] == 1 else {"number": "5", "url": "u5"}

        def ensure_label(self):
            pass

        def create_issue(self, title, body):
            state["create"] += 1
            raise bi.NetworkError("网络错误：SSLEOFError")

    sleeps = []
    action, issue = bi.record_issue(P(), "t", "b", now=NOW, sleep=sleeps.append)
    assert (action, issue["url"], state["create"], sleeps) == ("created", "u5", 1, [1])


# ───────────── HTTP 重试 ─────────────

class FakeResp:
    def __init__(self, status, body):
        self.status, self._body = status, body

    def read(self):
        return json.dumps(self._body).encode()

    def __enter__(self):
        return self

    def __exit__(self, *a):
        return False


def test_http_retries_ssl_eof_three_times_then_succeeds():
    errors = [ssl.SSLEOFError(8, "EOF occurred in violation of protocol"), urllib.error.URLError(ConnectionResetError()), TimeoutError()]
    seen = []

    def opener(req, timeout):
        seen.append(req.full_url)
        if errors:
            raise errors.pop(0)
        return FakeResp(201, {"ok": 1})

    sleeps = []
    http = bi.Http(secrets=[TOKEN], sleep=sleeps.append, opener=opener)
    assert http.request("GET", "https://x/api?access_token=" + TOKEN) == (201, {"ok": 1})
    assert len(seen) == 4 and sleeps == [1, 2, 3]


def test_http_gives_up_after_three_retries_without_leaking_token():
    def opener(req, timeout):
        raise urllib.error.URLError(f"连不上 {req.full_url}")

    http = bi.Http(secrets=[TOKEN], sleep=lambda s: None, opener=opener)
    with pytest.raises(bi.NetworkError) as e:
        http.request("GET", "https://gitee.com/api/v5/user?access_token=" + TOKEN)
    assert TOKEN not in str(e.value) and "已重试 3 次" in str(e.value)


def test_http_no_retry_for_create_and_4xx():
    calls = []

    def opener(req, timeout):
        calls.append(1)
        raise ssl.SSLEOFError(8, "EOF")

    with pytest.raises(bi.NetworkError):
        bi.Http(sleep=lambda s: None, opener=opener).request("POST", "https://x", json_body={}, retry=False)
    assert len(calls) == 1

    def opener404(req, timeout):
        calls.append(2)
        raise urllib.error.HTTPError(req.full_url, 404, "Not Found", {}, io.BytesIO(b'{"message":"Not Found Project"}'))

    calls.clear()
    with pytest.raises(bi.ApiError) as e:
        bi.Http(sleep=lambda s: None, opener=opener404).request("GET", "https://x")
    assert e.value.status == 404 and "Not Found Project" in str(e.value) and len(calls) == 1


def test_http_retries_502_then_succeeds():
    seq = [urllib.error.HTTPError("u", 502, "Bad Gateway", {}, io.BytesIO(b"")), FakeResp(200, [])]

    def opener(req, timeout):
        x = seq.pop(0)
        if isinstance(x, Exception):
            raise x
        return x

    assert bi.Http(sleep=lambda s: None, opener=opener).request("GET", "https://x") == (200, [])


# ───────────── 入口：token、退出码、输出 ─────────────

def write_report(tmp_path, text="## 导出报表 500\n\n**复现步骤**\n\n1. 点导出\n\n**修复建议**\n\n1. a.go:12\n"):
    f = tmp_path / "2026-11-19-export.md"
    f.write_text(text, encoding="utf-8")
    return f


def runtime(project=None):
    return {"projects": {"demo": project or {"repo": "git@gitee.com:example/demo-issues.git", "record": {"issue": True, "feishu_doc": True}}}}


def run_main(tmp_path, *, rt=None, provider=None, argv=None):
    out, err = io.StringIO(), io.StringIO()
    made = {}

    def make(target, token, http):
        made.update(target=target, token=token)
        return provider

    f = write_report(tmp_path)
    code = bi.main(argv or ["--project", "demo", "--file", str(f)], load_runtime=lambda: rt or runtime(),
                   make_provider=make, out=out, err=err, now=NOW)
    return code, out.getvalue(), err.getvalue(), made


class StubProvider:
    def __init__(self, existing=None, fail=None):
        self.existing, self.fail, self.created = existing, fail, []

    def find_open_issue(self, title):
        if self.fail:
            raise self.fail
        return self.existing

    def ensure_label(self):
        pass

    def create_issue(self, title, body):
        self.created.append((title, body))
        return {"number": "IKWMBW", "url": "https://gitee.com/x/IKWMBW"}

    def comment(self, issue, body):
        self.commented = body


def test_missing_token_exits_3_with_chinese_message(tmp_path):
    code, out, err, made = run_main(tmp_path, provider=StubProvider())
    assert code == bi.EXIT_NO_TOKEN == 3
    assert "未配置 GITEE_TOKEN" in err and out == "" and not made


def test_custom_token_env_name_in_message(tmp_path):
    rt = runtime({"repo": "https://github.com/o/r", "token_env": "MY_TOKEN"})
    code, _, err, _ = run_main(tmp_path, rt=rt, provider=StubProvider())
    assert code == 3 and "未配置 MY_TOKEN" in err


def test_success_prints_link_and_uses_title_and_body(tmp_path, monkeypatch):
    monkeypatch.setenv("GITEE_TOKEN", TOKEN)
    p = StubProvider()
    code, out, err, made = run_main(tmp_path, provider=p)
    assert code == 0 and out.strip() == "已创建 issue：https://gitee.com/x/IKWMBW"
    assert made["token"] == TOKEN and made["target"].full == "example/demo-issues"
    title, body = p.created[0]
    assert title == "导出报表 500" and body.startswith("**复现步骤**") and "## " not in body
    assert TOKEN not in out + err


def test_existing_issue_prints_comment_link(tmp_path, monkeypatch):
    monkeypatch.setenv("GITEE_TOKEN", TOKEN)
    code, out, _, _ = run_main(tmp_path, provider=StubProvider(existing={"number": "A1", "url": "https://gitee.com/x/A1"}))
    assert code == 0 and out.strip() == "已在 https://gitee.com/x/A1 追加评论"


def test_token_never_appears_in_any_output(tmp_path, monkeypatch):
    monkeypatch.setenv("GITEE_TOKEN", TOKEN)
    for fail in (bi.ApiError(500, f"接口返回 500：bad token {TOKEN}"),
                 bi.NetworkError(f"网络错误 https://gitee.com/api/v5/x?access_token={TOKEN}&a=1"),
                 RuntimeError(f"意外 {TOKEN}")):
        code, out, err, _ = run_main(tmp_path, provider=StubProvider(fail=fail))
        assert code == bi.EXIT_API
        assert TOKEN not in out + err and "SECRET" not in out + err, err
        assert err.startswith("[失败]")


def test_401_hint_names_env_var_without_token(tmp_path, monkeypatch):
    monkeypatch.setenv("GITEE_TOKEN", TOKEN)
    code, _, err, _ = run_main(tmp_path, provider=StubProvider(fail=bi.ApiError(401, "接口返回 401")))
    assert code == 4 and "GITEE_TOKEN 无效或已过期" in err and TOKEN not in err


def test_user_level_env_wins_over_stale_process_env(monkeypatch):
    monkeypatch.setenv("GITEE_TOKEN", "old-token")
    monkeypatch.setattr(bi, "_read_user_env", lambda name: "new-token" if name == "GITEE_TOKEN" else None)
    assert bi.read_token("GITEE_TOKEN") == "new-token"
    monkeypatch.setattr(bi, "_read_user_env", lambda name: None)
    assert bi.read_token("GITEE_TOKEN") == "old-token"
    monkeypatch.setenv("GITEE_TOKEN", "  ")
    assert bi.read_token("GITEE_TOKEN") is None


def test_unknown_project_bad_report_and_unsupported_repo_exit_2(tmp_path):
    out, err = io.StringIO(), io.StringIO()
    assert bi.main(["--project", "nope", "--file", "x.md"], load_runtime=runtime, out=out, err=err) == 2
    assert "未登记的项目" in err.getvalue()
    f = write_report(tmp_path, "没有标题\n正文")
    err = io.StringIO()
    assert bi.main(["--project", "demo", "--file", str(f)], load_runtime=runtime, out=out, err=err) == 2
    assert "第一行必须是" in err.getvalue()
    err = io.StringIO()
    rt = runtime({"repo": "git@gitlab.com:a/b.git"})
    assert bi.main(["--project", "demo", "--file", str(f)], load_runtime=lambda: rt, out=out, err=err) == 2
    assert "gitee.com / github.com" in err.getvalue()


def test_plan_prints_record_config_without_network(monkeypatch):
    out = io.StringIO()
    assert bi.main(["--project", "demo", "--plan"], load_runtime=runtime, out=out) == 0
    text = out.getvalue()
    assert "建 issue=是（Gitee · example/demo-issues，token 环境变量 GITEE_TOKEN：未检测到）" in text
    assert "写飞书文档=是" in text
    monkeypatch.setenv("GITEE_TOKEN", TOKEN)
    out = io.StringIO()
    rt = runtime({"repo": "git@gitee.com:a/b.git", "record": {"issue": True, "feishu_doc": False}})
    bi.main(["--project", "demo", "--plan"], load_runtime=lambda: rt, out=out)
    assert "已检测到" in out.getvalue() and "写飞书文档=否" in out.getvalue() and TOKEN not in out.getvalue()


def test_redact_masks_query_token_bearer_and_url_credentials():
    s = bi.redact(f"GET https://u:p@gitee.com/x?access_token=abc&x=1 Authorization: Bearer ghp_zzz {TOKEN}", [TOKEN])
    assert "abc" not in s and "ghp_zzz" not in s and TOKEN not in s and "u:p@" not in s


# ───────────── 两份脚本保持一致 ─────────────

def test_catalog_copy_is_identical_and_scripts_exist():
    assert (CATALOG / "_shared" / "bp_issue.py").read_bytes() == (SKILLS / "_shared" / "bp_issue.py").read_bytes()
    assert (CATALOG / "scripts" / "create_issue.py").is_file()
    assert (SKILLS / "issue-record" / "scripts" / "create_issue.py").is_file()
    manifest = json.loads((SKILLS / "issue-record" / "beetle.skill.json").read_text(encoding="utf-8"))
    assert manifest["name"] == "issue-record" and manifest["requires"] == ["python"]


@pytest.mark.parametrize("script", [
    SKILLS / "issue-record" / "scripts" / "create_issue.py",
    CATALOG / "scripts" / "create_issue.py",
])
def test_create_issue_script_entry_runs_plan(tmp_path, monkeypatch, capsys, script):
    """脚本入口：能找到 _shared，--plan 走通（运行时配置经 BEETLE_RUNTIME 给出；--plan 不联网，只打印是否检测到 token）。"""
    import runpy

    rt = tmp_path / "rt.json"
    rt.write_text(json.dumps(runtime(), ensure_ascii=False), encoding="utf-8")
    monkeypatch.delenv("BUG_PATROL_RUNTIME", raising=False)
    monkeypatch.setenv("BEETLE_RUNTIME", str(rt))
    monkeypatch.setattr(sys, "path", list(sys.path))  # 脚本会往 sys.path 插 _shared，测完还原
    for m in ("bp_common", "bp_issue"):
        monkeypatch.delitem(sys.modules, m, raising=False)
    monkeypatch.setattr(sys, "argv", [str(script), "--project", "demo", "--plan"])
    with pytest.raises(SystemExit) as e:
        runpy.run_path(str(script), run_name="__main__")
    assert e.value.code == 0
    out = capsys.readouterr().out
    assert "项目 demo 记录方式：建 issue=是（Gitee · example/demo-issues" in out
    for m in ("bp_common", "bp_issue"):
        sys.modules.pop(m, None)  # 不把这次加载的模块留给别的测试
