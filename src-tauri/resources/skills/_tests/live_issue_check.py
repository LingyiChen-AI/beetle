"""建 issue 真实联调（不进自动测试：会真的在仓库里建 issue、追加评论、关闭 issue）。

  python live_issue_check.py --repo-url https://gitee.com/example/demo-issues
  python live_issue_check.py --repo-url https://github.com/example/demo-issues
  python live_issue_check.py --repo-url <仓库地址> --token-env MY_TOKEN     # token 不在默认变量名里时

步骤：
  0. 用 token 查当前账号（验证 token 可用）
  1. 建一条「[联调] …」issue（带 bug 标签）
  2. 同标题再跑一次 → 应识别为已存在，改成追加评论（验证去重）
  3. 读 issue 详情，核对已打上 bug 标签
  4. 关闭 issue（前面任何一步失败，只要 issue 建出来了也会关闭）

token 读法与技能脚本一致：Windows 优先读用户级环境变量（HKCU\\Environment）的最新值，再读当前进程环境变量。
token 不会出现在任何输出里。全部通过退出码 0，否则 1；token 没配置 3。
"""
import argparse
import sys
from datetime import datetime
from pathlib import Path

for _s in (sys.stdout, sys.stderr):
    try:
        _s.reconfigure(encoding="utf-8")
    except Exception:
        pass

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "_shared"))
import bp_issue as bi  # noqa: E402


def main():
    ap = argparse.ArgumentParser(description="建 issue 真实联调：建 → 去重变评论 → 核对标签 → 关闭")
    ap.add_argument("--repo-url", required=True, help="仓库地址，如 https://gitee.com/owner/repo")
    ap.add_argument("--token-env", help="token 环境变量名，默认 Gitee 用 GITEE_TOKEN、GitHub 用 GITHUB_TOKEN")
    a = ap.parse_args()

    try:
        target = bi.resolve_target({"repo": a.repo_url, "token_env": a.token_env})
    except bi.IssueError as e:
        print(f"[失败] {e}")
        return 2
    token = bi.read_token(target.token_env)
    if not token:
        print(f"[失败] 未配置 {target.token_env}")
        return 3
    provider = bi.build_provider(target, token, bi.Http(secrets=[token]))
    now = datetime.now()
    title = f"[联调] beetle 建 issue 联调 {now:%Y%m%d-%H%M%S}"
    body = (
        "**复现步骤**\n\n1. 联调脚本自动创建，用于验证建 issue、去重追加评论、打 bug 标签、关闭。\n\n"
        "**修复建议**\n\n1. 无（联调用，脚本结束时自动关闭）\n\n"
        "**数据依据**：无\n\n"
        f"**提出人**：联调脚本\n\n**提出时间**：{now:%Y-%m-%d %H:%M}\n\n"
        "**代码版本**：无"
    )
    results = []

    def step(n, ok, msg):
        results.append(ok)
        print(f"[{n}] {'通过' if ok else '未通过'}：{bi.redact(msg, [token])}", flush=True)

    print(f"目标：{target.label}，token 环境变量 {target.token_env}，标题「{title}」", flush=True)
    number = None
    try:
        user = provider.whoami()
        step(0, bool(user), f"token 对应账号 {user}")

        action, issue = bi.record_issue(provider, title, body)
        number = issue["number"] or None
        step(1, action == "created" and bool(number), f"第一次：{action}，{issue['url']}（编号 {issue['number']}）")

        action2, issue2 = bi.record_issue(provider, title, body)
        step(2, action2 == "commented" and issue2["number"] == issue["number"],
             f"第二次：{action2}，{issue2['url']}（应为 commented 且编号与第一次相同）")

        labels = provider.labels_of(issue["number"])
        step(3, bi.LABEL in labels, f"issue 标签：{labels}")
    except bi.IssueError as e:
        step("x", False, f"出错：{e}")
    except Exception as e:  # 兜底：任何意外都不能把 token 打出去
        step("x", False, f"意外错误 {type(e).__name__}: {e}")
    finally:
        if number:
            try:
                provider.close_issue(number)
                step(4, True, f"已关闭 issue {number}")
            except Exception as e:
                step(4, False, f"关闭 issue {number} 失败：{e}（请手动关闭）")

    ok = bool(results) and all(results)
    print("联调结果：" + ("全部通过" if ok else "有未通过的步骤"), flush=True)
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
