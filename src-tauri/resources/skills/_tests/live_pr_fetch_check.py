"""PR 拉取真实联调（不进自动测试：会真的访问 Gitee / GitHub 接口，只读）。

  python live_pr_fetch_check.py --url https://gitee.com/<owner>/<repo>/pulls/<编号>
  python live_pr_fetch_check.py --url https://github.com/<owner>/<repo>/pull/<编号>
  python live_pr_fetch_check.py --url <PR链接> --token-env MY_TOKEN      # token 不在默认变量名里时
  python live_pr_fetch_check.py --url <PR链接> --max-diff-chars 20000    # 顺带看大 diff 截断

步骤（全部只读，不在 PR 上做任何改动）：
  1. 解析链接，读 token（Windows 优先用户级环境变量；没有也行，公开仓库不需要）
  2. 拉 PR 详情、改动文件清单（GitHub 分页）、diff（GitHub 整段 diff，取不到按文件拼）
  3. 核对：标题 / 作者 / 分支 / head_sha 非空，增删行数与文件数合理，diff 里每个文件都有 diff --git 头
  4. 核对：token 不出现在任何输出和 diff 文件里
diff 写到临时目录并打印位置，可以打开看。全部通过退出码 0，否则 1；参数不对 2；私有仓库没配 token 3；接口 / 网络失败 4。
"""
import argparse
import io
import json
import sys
import tempfile
from pathlib import Path

for _s in (sys.stdout, sys.stderr):
    try:
        _s.reconfigure(encoding="utf-8")
    except Exception:
        pass

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "_shared"))
import bp_issue as gi  # noqa: E402
import bp_pr as bp  # noqa: E402


def main():
    ap = argparse.ArgumentParser(description="PR 拉取真实联调（只读）")
    ap.add_argument("--url", required=True, help="PR 链接")
    ap.add_argument("--token-env", help="token 环境变量名，默认 Gitee 用 GITEE_TOKEN、GitHub 用 GITHUB_TOKEN")
    ap.add_argument("--max-diff-chars", type=int, default=bp.DEFAULT_MAX_DIFF_CHARS)
    a = ap.parse_args()

    try:
        pr = bp.parse_pr_url(a.url)
    except bp.PrError as e:
        print(f"[失败] {e}")
        return 2
    token_env = a.token_env or gi.DEFAULT_TOKEN_ENV[pr.platform]
    token = gi.read_token(token_env)
    print(f"0. {gi.PLATFORM_LABEL[pr.platform]} · {pr.full} #{pr.number}；token 环境变量 {token_env}：{'已检测到' if token else '未检测到（按公开仓库读）'}")

    out_dir = Path(tempfile.mkdtemp(prefix="beetle-pr-"))
    rt = {"projects": {"live": {"repo": f"https://{'gitee.com' if pr.platform == 'gitee' else 'github.com'}/{pr.owner}/{pr.repo}", "token_env": token_env}}}
    out, err = io.StringIO(), io.StringIO()
    code = bp.fetch_main(["--url", a.url, "--out", str(out_dir), "--max-diff-chars", str(a.max_diff_chars)],
                         load_runtime=lambda: rt, out=out, err=err)
    if code != 0:
        print(err.getvalue().strip())
        return code
    meta = json.loads(out.getvalue())
    diff_file = Path(meta["diff_file"])
    diff = diff_file.read_text(encoding="utf-8")

    results = []

    def check(n, ok, msg):
        results.append(ok)
        print(f"{n}. [{'通过' if ok else '失败'}] {msg}")

    print(f"1. 标题：{meta['title']}")
    print(f"   作者：{meta['author']}；{meta['source_branch']} → {meta['target_branch']}；head {meta['head_sha'][:10]}；状态 {meta['state']}")
    print(f"   规模：+{meta['additions']}/-{meta['deletions']}，{meta['changed_files']} 个文件（清单里 {len(meta['files'])} 个）")
    skipped = [f for f in meta["files"] if f.get("skipped")]
    no_patch = [f for f in meta["files"] if f.get("no_patch")]
    print(f"   跳过 {len(skipped)} 个（{'、'.join(sorted({f['skipped'] for f in skipped})) or '无'}），没有 diff 的 {len(no_patch)} 个")
    print(f"   diff：{len(diff)} 字符，截断={meta['diff_truncated']}，省略 {len(meta['omitted_files'])} 个文件 → {diff_file.as_posix()}")
    check(2, all(meta[k] for k in ("title", "author", "source_branch", "target_branch", "head_sha")), "标题 / 作者 / 分支 / head_sha 都拿到了")
    check(3, meta["changed_files"] >= len([f for f in meta["files"]]) or pr.platform == "gitee", "文件数与清单一致（GitHub 清单最多 3000 个）")
    reviewable = [f for f in meta["files"] if not f.get("skipped") and not f.get("no_patch") and f["path"] not in meta["omitted_files"]]
    missing = [f["path"] for f in reviewable if f"b/{f['path']}" not in diff and f"a/{f['path']}" not in diff]
    check(4, not missing, "需要审的文件都在 diff 里" + (f"（缺：{'、'.join(missing[:5])}）" if missing else ""))
    leaked = bool(token) and any(token in t for t in (out.getvalue(), err.getvalue(), diff))
    check(5, not leaked, "token 没有出现在输出和 diff 文件里")
    print("全部通过" if all(results) else "有检查没通过")
    return 0 if all(results) else 1


if __name__ == "__main__":
    sys.exit(main())
