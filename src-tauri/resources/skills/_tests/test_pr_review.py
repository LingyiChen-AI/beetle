"""PR 审查（_shared/bp_pr.py + pr-review 脚本）测试：全部用假 HTTP，不访问网络、不读真实 token。

bp_pr 直接复用 bp_issue 的仓库解析 / token / HTTP / 打码，这里先把 skills/_shared/bp_issue.py 以模块名 bp_issue 装好，再加载 bp_pr。
技能中心 code-review 带的 bp_pr.py / bp_issue.py / 两个脚本要和机器人内置版完全相同。
"""
import importlib.util
import io
import json
import runpy
import sys
import urllib.parse
from pathlib import Path

import pytest

SKILLS = Path(__file__).resolve().parents[1]
CATALOG = SKILLS.parent / "catalog" / "code-review"
SHARED = SKILLS / "_shared"


def _load(path, name):
    spec = importlib.util.spec_from_file_location(name, path)
    mod = importlib.util.module_from_spec(spec)
    sys.modules[name] = mod
    spec.loader.exec_module(mod)
    return mod


gi = _load(SHARED / "bp_issue.py", "bp_issue")
bp = _load(SHARED / "bp_pr.py", "bp_pr")

TOKEN = "tok-SECRET-123456"
GITEE_URL = "https://gitee.com/example/demo-service/pulls/29"
GITHUB_URL = "https://github.com/example/demo/pull/7"


@pytest.fixture(autouse=True)
def no_real_token(monkeypatch):
    """不读开发机上真实的用户级环境变量 / 进程环境变量。"""
    monkeypatch.setattr(gi, "_read_user_env", lambda name: None)
    for k in ("GITEE_TOKEN", "GITHUB_TOKEN", "MY_TOKEN"):
        monkeypatch.delenv(k, raising=False)


class FakeHttp:
    """按 (方法, 路径) 返回预设结果；handler 可以是值、异常或函数（参数是这次调用）。记录每次调用。"""

    def __init__(self, routes):
        self.routes = routes
        self.calls = []

    def sleep(self, seconds):
        pass

    def request(self, method, url, *, headers=None, form=None, json_body=None, retry=True, raw=False):
        u = urllib.parse.urlsplit(url)
        call = {"method": method, "path": u.path, "query": dict(urllib.parse.parse_qsl(u.query)),
                "headers": headers or {}, "raw": raw}
        self.calls.append(call)
        key = (method, u.path, "raw") if raw else (method, u.path)
        h = self.routes.get(key)
        if h is None:
            raise AssertionError(f"没有预设 {key}")
        if callable(h):
            h = h(call)
        if isinstance(h, Exception):
            raise h
        return 200, h


GITHUB_DETAIL = {
    "title": "feat: 售后工单自动领取", "user": {"login": "zhangsan"}, "html_url": GITHUB_URL,
    "head": {"ref": "feature/claim", "sha": "abc1234def"}, "base": {"ref": "master", "sha": "0001112"},
    "state": "open", "merged": False, "additions": 130, "deletions": 9, "changed_files": 3,
}
GITHUB_FILES = [
    {"filename": "src/claim.go", "status": "modified", "additions": 120, "deletions": 9, "patch": "@@ -1,2 +1,3 @@\n a\n+b\n"},
    {"filename": "package-lock.json", "status": "modified", "additions": 10, "deletions": 0, "patch": "@@ -1 +1 @@\n-x\n+y\n"},
    {"filename": "docs/logo.png", "status": "added", "additions": 0, "deletions": 0},
]
GITHUB_DIFF = ("diff --git a/src/claim.go b/src/claim.go\nindex 1..2 100644\n--- a/src/claim.go\n+++ b/src/claim.go\n@@ -1,2 +1,3 @@\n a\n+b\n"
               "diff --git a/package-lock.json b/package-lock.json\n--- a/package-lock.json\n+++ b/package-lock.json\n@@ -1 +1 @@\n-x\n+y\n")


def github_http():
    p = "/repos/example/demo/pulls/7"
    return FakeHttp({("GET", p): GITHUB_DETAIL, ("GET", p + "/files"): GITHUB_FILES, ("GET", p, "raw"): GITHUB_DIFF})


GITEE_DETAIL = {
    "title": "fix: 判重走主库", "user": {"login": "zhangsan", "name": "张三"}, "html_url": GITEE_URL,
    "head": {"ref": "hotfix/dedupe", "sha": "fedcba9876"}, "base": {"ref": "release/V3.2", "sha": "1234567"}, "state": "open",
}
GITEE_FILES = [
    {"filename": "app/dedupe.py", "status": "modified", "additions": "5", "deletions": "2",
     "patch": {"diff": "@@ -1 +1,2 @@\n-old\n+new\n+more\n", "old_path": "app/dedupe.py", "new_path": "app/dedupe.py"}},
    {"filename": "app/new_mod.py", "additions": "3", "deletions": "0",
     "patch": {"diff": "@@ -0,0 +1,3 @@\n+a\n+b\n+c\n", "new_file": True, "new_path": "app/new_mod.py", "old_path": "app/new_mod.py"}},
    {"filename": "big.sql", "additions": "9000", "deletions": "0", "patch": {"diff": "", "too_large": True}},
]


def gitee_http():
    p = "/api/v5/repos/example/demo-service/pulls/29"
    return FakeHttp({("GET", p): GITEE_DETAIL, ("GET", p + "/files"): GITEE_FILES})


# ---------- 链接与项目 ----------

@pytest.mark.parametrize("url,want", [
    (GITEE_URL, ("gitee", "example", "demo-service", 29)),
    (GITEE_URL + "/", ("gitee", "example", "demo-service", 29)),
    (GITEE_URL + "?tab=files#note", ("gitee", "example", "demo-service", 29)),
    ("http://www.github.com/Example/demo.svc/pull/7/files", ("github", "Example", "demo.svc", 7)),
])
def test_parse_pr_url_forms(url, want):
    p = bp.parse_pr_url(url)
    assert (p.platform, p.owner, p.repo, p.number) == want


@pytest.mark.parametrize("url", [
    "https://gitee.com/example/demo-service/pull/29", "https://github.com/example/demo/pulls/7",
    "https://github.com/example/demo/issues/7", "https://gitlab.com/a/b/merge_requests/1",
    "https://github.com/example/demo/pull/0", "not a url",
])
def test_parse_pr_url_rejects_non_pr(url):
    with pytest.raises(bp.PrError) as e:
        bp.parse_pr_url(url)
    assert e.value.code == bp.EXIT_USAGE


def test_match_project_by_platform_and_repo_and_token_env():
    rt = {"projects": {
        "svc": {"repo": "git@gitee.com:Example/Demo-Service.git", "token_env": "MY_TOKEN"},
        "gh": {"repo": "https://github.com/example/demo"},
        "other": {"repo": "git@x:a/b.git"},
    }}
    name, p = bp.match_project(rt, bp.parse_pr_url(GITEE_URL))
    assert name == "svc" and bp.token_env_for(bp.parse_pr_url(GITEE_URL), p) == "MY_TOKEN"
    assert bp.match_project(rt, bp.parse_pr_url(GITHUB_URL))[0] == "gh"
    assert bp.match_project(rt, bp.parse_pr_url("https://github.com/example/other/pull/1")) == (None, None)
    assert bp.token_env_for(bp.parse_pr_url(GITHUB_URL), None) == "GITHUB_TOKEN"


# ---------- 两个平台的请求与解析 ----------

def test_github_fetch_uses_diff_accept_and_bearer_token():
    http = github_http()
    pr = bp.parse_pr_url(GITHUB_URL)
    meta, diff = bp.fetch(pr, bp.build_client(pr, TOKEN, http))
    assert (meta["title"], meta["author"], meta["source_branch"], meta["target_branch"]) == ("feat: 售后工单自动领取", "zhangsan", "feature/claim", "master")
    assert (meta["head_sha"], meta["additions"], meta["deletions"], meta["changed_files"]) == ("abc1234def", 130, 9, 3)
    raw = [c for c in http.calls if c["raw"]]
    assert raw and raw[0]["headers"]["Accept"] == "application/vnd.github.diff"
    assert all(c["headers"].get("Authorization") == f"Bearer {TOKEN}" for c in http.calls)
    files = {f["path"]: f for f in meta["files"]}
    assert files["package-lock.json"]["skipped"] == "lock 文件"
    assert files["docs/logo.png"]["skipped"] == "二进制文件"
    assert "src/claim.go" in diff and "package-lock.json" not in diff, "跳过的文件不进 diff"
    assert "patch" not in files["src/claim.go"]
    # 没 token：不带 Authorization
    http2 = github_http()
    bp.fetch(pr, bp.build_client(pr, None, http2))
    assert all("Authorization" not in c["headers"] for c in http2.calls)


def test_github_too_large_diff_falls_back_to_file_patches():
    http = github_http()
    http.routes[("GET", "/repos/example/demo/pulls/7", "raw")] = gi.ApiError(406, "diff too large")
    pr = bp.parse_pr_url(GITHUB_URL)
    _, diff = bp.fetch(pr, bp.build_client(pr, None, http))
    assert diff.startswith("diff --git a/src/claim.go b/src/claim.go\n--- a/src/claim.go\n+++ b/src/claim.go\n@@ -1,2 +1,3 @@")


def test_github_files_are_paginated():
    page1 = [{"filename": f"f{i}.go", "status": "modified", "additions": 1, "deletions": 0, "patch": "@@ -1 +1 @@\n+x\n"} for i in range(100)]
    page2 = [{"filename": "last.go", "status": "added", "additions": 1, "deletions": 0, "patch": "@@ -0,0 +1 @@\n+y\n"}]
    pages = {"1": page1, "2": page2}
    http = github_http()
    http.routes[("GET", "/repos/example/demo/pulls/7/files")] = lambda call: pages[call["query"]["page"]]
    pr = bp.parse_pr_url(GITHUB_URL)
    meta, _ = bp.fetch(pr, bp.build_client(pr, None, http))
    assert len(meta["files"]) == 101 and meta["files"][-1]["path"] == "last.go"


def test_gitee_fetch_uses_access_token_and_assembles_diff():
    http = gitee_http()
    pr = bp.parse_pr_url(GITEE_URL)
    meta, diff = bp.fetch(pr, bp.build_client(pr, TOKEN, http))
    assert all(c["query"].get("access_token") == TOKEN for c in http.calls)
    assert (meta["author"], meta["source_branch"], meta["target_branch"], meta["head_sha"]) == ("张三", "hotfix/dedupe", "release/V3.2", "fedcba9876")
    # Gitee 详情里没有总数：按文件加总（字符串数字也认）
    assert (meta["additions"], meta["deletions"], meta["changed_files"]) == (9008, 2, 3)
    files = {f["path"]: f for f in meta["files"]}
    assert files["app/new_mod.py"]["status"] == "added"
    assert files["big.sql"].get("no_patch") is True
    assert "diff --git a/app/dedupe.py b/app/dedupe.py\n--- a/app/dedupe.py\n+++ b/app/dedupe.py\n@@ -1 +1,2 @@" in diff
    assert "--- /dev/null\n+++ b/app/new_mod.py" in diff
    # 没 token：不带 access_token
    http2 = gitee_http()
    bp.fetch(pr, bp.build_client(pr, None, http2))
    assert all("access_token" not in c["query"] for c in http2.calls)


# ---------- diff 截断与跳过 ----------

def test_build_diff_truncates_whole_files_and_lists_omitted():
    files = [
        {"path": "a.go", "status": "modified", "patch": "@@ -1 +1 @@\n+" + "a" * 50 + "\n"},
        {"path": "b.go", "status": "modified", "patch": "@@ -1 +1 @@\n+" + "b" * 500 + "\n"},
        {"path": "c.go", "status": "modified", "patch": "@@ -1 +1 @@\n+c\n"},
        {"path": "yarn.lock", "status": "modified", "patch": "@@ -1 +1 @@\n+z\n", "skipped": "lock 文件"},
    ]
    text, truncated, omitted = bp.build_diff(files, None, max_chars=300)
    assert truncated and omitted == ["b.go"]
    assert text.startswith("# diff 太大，以下文件没有放进来，请到代码目录里用 git 看：\n#   b.go\n")
    assert "a" * 50 in text and "+c\n" in text, "放得下的照常放（后面的小文件不受影响）"
    assert "b" * 500 not in text and "yarn.lock" not in text
    full, t2, o2 = bp.build_diff(files, None, max_chars=10_000)
    assert not t2 and o2 == [] and not full.startswith("#")


@pytest.mark.parametrize("path,reason", [
    ("web/package-lock.json", "lock 文件"), ("go.sum", "lock 文件"), ("web/node_modules/x/index.js", "依赖或构建产物目录"),
    ("static/app.min.js", "压缩过的前端产物"), ("api/user_pb2.py", "生成的代码"), ("img/a.PNG", "二进制文件"),
    ("src/build/builder.go", None), ("src/main.rs", None),
])
def test_skip_reason(path, reason):
    assert bp.skip_reason(path) == reason


# ---------- 命令行：token 不出现在输出里、错误提示 ----------

def run_fetch(tmp_path, argv, http, rt=None):
    out, err = io.StringIO(), io.StringIO()
    code = bp.fetch_main(argv, load_runtime=lambda: rt or {}, make_client=lambda pr, token, _h: bp.build_client(pr, token, http), out=out, err=err)
    return code, out.getvalue(), err.getvalue()


def test_fetch_main_writes_diff_file_and_token_never_appears(tmp_path, monkeypatch):
    monkeypatch.setenv("GITEE_TOKEN", TOKEN)
    http = gitee_http()
    # 让 PR 内容里也带上 token，验证打码
    http.routes[("GET", "/api/v5/repos/example/demo-service/pulls/29")] = dict(GITEE_DETAIL, title=f"带 {TOKEN} 的标题")
    rt = {"projects": {"svc": {"repo": "https://gitee.com/example/demo-service"}}}
    code, out, err = run_fetch(tmp_path, ["--url", GITEE_URL, "--out", str(tmp_path / "pr")], http, rt)
    assert code == 0, err
    meta = json.loads(out)
    assert meta["project"] == "svc" and meta["token_env"] == "GITEE_TOKEN"
    diff_file = Path(meta["diff_file"])
    assert diff_file.is_file() and diff_file.name == "gitee-example-demo-service-29.diff"
    for text in (out, err, diff_file.read_text(encoding="utf-8")):
        assert TOKEN not in text
    assert "diff" not in meta


def test_fetch_main_without_out_puts_diff_in_json(tmp_path):
    code, out, _ = run_fetch(tmp_path, ["--url", GITHUB_URL], github_http())
    assert code == 0
    meta = json.loads(out)
    assert "src/claim.go" in meta["diff"]
    assert "body" in meta and "body_file" not in meta


def test_fetch_main_writes_body_file_for_ocr_background(tmp_path, monkeypatch):
    monkeypatch.setenv("GITHUB_TOKEN", TOKEN)
    http = github_http()
    http.routes[("GET", "/repos/example/demo/pulls/7")] = dict(GITHUB_DETAIL, body=f"需求：领取要判重\n调试用 {TOKEN}")
    code, out, err = run_fetch(tmp_path, ["--url", GITHUB_URL, "--out", str(tmp_path / "pr")], http)
    assert code == 0, err
    meta = json.loads(out)
    bf = Path(meta["body_file"])
    assert bf.name == "github-example-demo-7.body.md" and bf.parent == Path(meta["diff_file"]).parent
    text = bf.read_text(encoding="utf-8")
    assert text.startswith("# feat: 售后工单自动领取\n") and "feature/claim → master" in text and "需求：领取要判重" in text
    assert TOKEN not in text and TOKEN not in out
    assert "body" not in meta, "描述只写文件，不进输出 JSON"
    # 没有描述也写文件，并写明
    code, out, _ = run_fetch(tmp_path, ["--url", GITHUB_URL, "--out", str(tmp_path / "pr2")], github_http())
    assert "（PR 没有描述）" in Path(json.loads(out)["body_file"]).read_text(encoding="utf-8")


def test_body_markdown_truncates_long_description():
    text = bp.body_markdown({"title": "t", "url": "u"}, "长" * (bp.MAX_BODY_CHARS + 50))
    assert "（描述过长，后面省略）" in text and text.count("长") <= bp.MAX_BODY_CHARS + 2


def test_fetch_main_private_repo_without_token_exits_3(tmp_path):
    http = gitee_http()
    http.routes[("GET", "/api/v5/repos/example/demo-service/pulls/29")] = gi.ApiError(404, "接口返回 404")
    code, out, err = run_fetch(tmp_path, ["--url", GITEE_URL], http)
    assert code == bp.EXIT_NO_TOKEN and out == ""
    assert "私有" in err and "GITEE_TOKEN" in err


def test_fetch_main_bad_token_exits_4_without_leaking(tmp_path, monkeypatch):
    monkeypatch.setenv("GITHUB_TOKEN", TOKEN)
    http = github_http()
    http.routes[("GET", "/repos/example/demo/pulls/7")] = gi.ApiError(401, f"接口返回 401：bad {TOKEN}")
    code, out, err = run_fetch(tmp_path, ["--url", GITHUB_URL], http)
    assert code == bp.EXIT_API
    assert "GITHUB_TOKEN 无效或已过期" in err and TOKEN not in err + out


def test_fetch_main_network_error_and_bad_url(tmp_path):
    http = github_http()
    http.routes[("GET", "/repos/example/demo/pulls/7")] = gi.NetworkError("网络错误（已重试 3 次）：SSLError: EOF")
    assert run_fetch(tmp_path, ["--url", GITHUB_URL], http)[0] == bp.EXIT_API
    code, _, err = run_fetch(tmp_path, ["--url", "https://github.com/example/demo/issues/1"], http)
    assert code == bp.EXIT_USAGE and "不是 Gitee / GitHub 的 PR 链接" in err


def test_user_level_token_is_used(monkeypatch, tmp_path):
    monkeypatch.setattr(gi, "_read_user_env", lambda name: "user-level-token" if name == "GITHUB_TOKEN" else None)
    seen = []
    http = github_http()
    code = bp.fetch_main(["--url", GITHUB_URL], load_runtime=lambda: {}, out=io.StringIO(), err=io.StringIO(),
                         make_client=lambda pr, token, _h: seen.append(token) or bp.build_client(pr, token, http))
    assert code == 0 and seen == ["user-level-token"]


# ---------- 上一轮审查结果 ----------

RESULT = {"repo": "demo-service", "pr": 29, "title": "t", "conclusion": "c",
          "findings": [{"severity": "medium", "status": "new", "text": "判重走了从库 b.go:40"}],
          "additions": 9, "deletions": 2, "files": 3, "author": "张三", "url": GITEE_URL, "head_sha": "fedcba9876"}


def test_review_state_save_and_get_roundtrip(tmp_path):
    rt = {"review_state_dir": str(tmp_path / "reviews")}
    pr = bp.parse_pr_url("https://gitee.com/Example/Demo-Service/pulls/29")
    assert bp.load_state(rt, pr) is None
    p = bp.save_state(rt, pr, RESULT)
    assert p == tmp_path / "reviews" / "gitee" / "example" / "demo-service" / "29.json", "owner / repo 小写，和客户端同一位置"
    got = bp.load_state(rt, bp.parse_pr_url(GITEE_URL + "#x"))
    assert got["findings"] == RESULT["findings"] and got["saved_at"] and got["pr_url"].endswith("/pulls/29")
    assert not list(p.parent.glob("*.tmp"))


def test_review_state_main_get_save_and_validation(tmp_path):
    rt = {"review_state_dir": str(tmp_path / "reviews")}

    def run(argv):
        out, err = io.StringIO(), io.StringIO()
        return bp.state_main(argv, load_runtime=lambda: rt, out=out, err=err), out.getvalue(), err.getvalue()

    assert run(["get", "--url", GITEE_URL])[1].strip() == bp.NO_PREVIOUS
    f = tmp_path / "r.json"
    f.write_text(json.dumps(RESULT, ensure_ascii=False), encoding="utf-8")
    code, out, _ = run(["save", "--url", GITEE_URL, "--file", str(f)])
    assert code == 0 and out.startswith("已保存：")
    code, out, _ = run(["get", "--url", GITEE_URL])
    assert code == 0 and json.loads(out)["head_sha"] == "fedcba9876"
    assert run(["path", "--url", GITEE_URL])[1].strip().endswith("gitee/example/demo-service/29.json")
    for bad in ({"repo": "", "pr": 1, "findings": []}, {"repo": "a", "pr": 1, "findings": [{"severity": "致命", "text": "x"}]},
                {"repo": "a", "pr": 1, "findings": [{"severity": "low", "status": "done", "text": "x"}]}, {"repo": "a", "pr": 1}):
        f.write_text(json.dumps(bad, ensure_ascii=False), encoding="utf-8")
        code, _, err = run(["save", "--url", GITEE_URL, "--file", str(f)])
        assert code == bp.EXIT_USAGE and err.startswith("[失败]"), bad
    f.write_text("{坏的", encoding="utf-8")
    assert run(["save", "--url", GITEE_URL, "--file", str(f)])[0] == bp.EXIT_USAGE


COVERAGE = {"total": 6, "reviewed": 5, "skipped": [{"path": "db/big.sql", "reason": "diff 取不到"}]}


def test_review_state_save_keeps_coverage_as_is(tmp_path):
    rt = {"review_state_dir": str(tmp_path / "reviews")}
    pr = bp.parse_pr_url(GITEE_URL)
    bp.save_state(rt, pr, dict(RESULT, coverage=COVERAGE))
    assert bp.load_state(rt, pr)["coverage"] == COVERAGE
    # 没有 coverage 也能存（可缺省）
    bp.save_state(rt, pr, RESULT)
    assert "coverage" not in bp.load_state(rt, pr)


@pytest.mark.parametrize("cov", [
    "全部",
    {"total": 3, "reviewed": 4, "skipped": []},
    {"total": "6", "reviewed": 5},
    {"total": 6, "reviewed": -1},
    {"total": 6, "reviewed": 5, "skipped": "a.sql"},
    {"total": 6, "reviewed": 5, "skipped": [{"reason": "没写路径"}]},
])
def test_review_state_rejects_bad_coverage(tmp_path, cov):
    with pytest.raises(bp.PrError) as e:
        bp.save_state({"review_state_dir": str(tmp_path)}, bp.parse_pr_url(GITEE_URL), dict(RESULT, coverage=cov))
    assert e.value.code == bp.EXIT_USAGE and "coverage" in str(e.value)


def test_state_root_defaults_without_runtime():
    assert bp.state_root({}) == Path.home() / ".beetle" / "pr-reviews"


# ---------- 脚本入口与技能中心副本 ----------

def test_state_script_entry_reads_runtime_env(tmp_path, monkeypatch, capsys):
    rt = tmp_path / "rt.json"
    rt.write_text(json.dumps({"review_state_dir": str(tmp_path / "reviews"), "projects": {}}), encoding="utf-8")
    monkeypatch.delenv("BUG_PATROL_RUNTIME", raising=False)
    monkeypatch.setenv("BEETLE_RUNTIME", str(rt))
    monkeypatch.setattr(sys, "path", list(sys.path))
    for m in ("bp_common",):
        monkeypatch.delitem(sys.modules, m, raising=False)
    monkeypatch.setattr(sys, "argv", ["pr_review_state.py", "get", "--url", GITEE_URL])
    with pytest.raises(SystemExit) as e:
        runpy.run_path(str(SKILLS / "pr-review" / "scripts" / "pr_review_state.py"), run_name="__main__")
    assert e.value.code == 0
    assert capsys.readouterr().out.strip() == bp.NO_PREVIOUS


def test_catalog_copy_is_identical_and_complete():
    for rel in ("_shared/bp_pr.py", "_shared/bp_issue.py"):
        assert (CATALOG / rel).read_bytes() == (SKILLS / rel).read_bytes(), rel
    for name in ("pr_fetch.py", "pr_review_state.py"):
        assert (CATALOG / "scripts" / name).read_bytes() == (SKILLS / "pr-review" / "scripts" / name).read_bytes(), name
    assert (SKILLS.parent / "catalog" / "bug-hunt" / "_shared" / "bp_issue.py").read_bytes() == (SKILLS / "_shared" / "bp_issue.py").read_bytes()
    for f in ("catalog.json", "SKILL.md", "scripts/sync_code.py", "_shared/bp_common.py"):
        assert (CATALOG / f).is_file(), f
    assert json.loads((CATALOG / "catalog.json").read_text(encoding="utf-8"))["name"] == "code-review"
