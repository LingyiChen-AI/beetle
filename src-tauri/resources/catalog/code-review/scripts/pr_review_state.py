"""读取 / 保存某个 PR 上一轮的代码审查结果（增量复审用）。

  python pr_review_state.py get  --url <PR链接>                 # 打印上一轮结果 JSON；没有打印「无上一轮审查记录」
  python pr_review_state.py save --url <PR链接> --file r.json   # 保存本轮结果（机器人运行里由客户端保存，不用调）
  python pr_review_state.py path --url <PR链接>                 # 打印结果文件位置

结果按 PR 存在运行时配置的 review_state_dir 下：<平台>/<owner>/<repo>/<编号>.json（owner / repo 小写），和客户端同一个位置。
退出码：0 成功；2 参数或结果格式不对。逻辑在 _shared/bp_pr.py。
"""
import sys
from pathlib import Path

_here = Path(__file__).resolve()
for _cand in (_here.parents[1] / "_shared", _here.parents[2] / "_shared"):
    if (_cand / "bp_pr.py").is_file():
        sys.path.insert(0, str(_cand))
        break

from bp_common import load_runtime  # noqa: E402
import bp_pr  # noqa: E402

if __name__ == "__main__":
    sys.exit(bp_pr.state_main(load_runtime=load_runtime))
