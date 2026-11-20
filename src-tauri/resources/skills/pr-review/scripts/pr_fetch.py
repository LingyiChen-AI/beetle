"""拉取 Gitee / GitHub PR 的元数据、改动文件清单和 diff（只读，不在 PR 上做任何改动）。

  python pr_fetch.py --url https://gitee.com/<owner>/<repo>/pulls/<编号> --out reports/pr
  python pr_fetch.py --url https://github.com/<owner>/<repo>/pull/<编号>            # 不给 --out：diff 放进输出 JSON

标准输出是一段 JSON：platform、owner、repo、number、url、title、author、source_branch、target_branch、head_sha、base_sha、
state、additions、deletions、changed_files、project（运行时配置里仓库一致的项目名，可能为 null）、token_env、
files（path、status、additions、deletions，跳过的带 skipped 原因）、diff_truncated、omitted_files、
diff_file + body_file（PR 描述，给 ocr -B 用；有 --out 时）或 diff + body（没有 --out 时）。
退出码：0 成功；2 不是 PR 链接；3 读不到且没配 token（多半是私有仓库）；4 接口或网络失败。逻辑在 _shared/bp_pr.py。
"""
import sys
from pathlib import Path

_here = Path(__file__).resolve()
# 机器人运行时：<skills>/pr-review/scripts → <skills>/_shared；技能中心：<技能目录>/scripts → <技能目录>/_shared
for _cand in (_here.parents[1] / "_shared", _here.parents[2] / "_shared"):
    if (_cand / "bp_pr.py").is_file():
        sys.path.insert(0, str(_cand))
        break

from bp_common import load_runtime  # noqa: E402
import bp_pr  # noqa: E402

if __name__ == "__main__":
    sys.exit(bp_pr.fetch_main(load_runtime=load_runtime))
