"""把 reports/ 下的 bug 报告建成 Gitee / GitHub issue；同标题的 open issue 已存在就追加评论。

  python create_issue.py --project <项目名> --file reports/<文件>.md
  python create_issue.py --project <项目名> --plan      # 只看记录方式和 token 是否已配置，不联网

报告第一行 `## 标题` 作为 issue 标题，其余为正文。仓库、平台、token 环境变量名取运行时配置（BEETLE_RUNTIME）里的项目。
成功打印「已创建 issue：<链接>」或「已在 <链接> 追加评论」。
退出码：0 成功；2 参数 / 配置 / 报告格式不对；3 未配置 token；4 接口或网络失败。逻辑在 _shared/bp_issue.py。
"""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "_shared"))
from bp_common import load_runtime  # noqa: E402
import bp_issue  # noqa: E402

if __name__ == "__main__":
    sys.exit(bp_issue.main(load_runtime=load_runtime))
