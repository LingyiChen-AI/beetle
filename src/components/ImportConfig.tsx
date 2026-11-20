// 导入同事配置：填文件路径 → preview_import 预览 → 勾选要导入的项目 / 数据源 / 模板 / 技能 → apply_import。
// 设置页与首次引导共用。导入文件不含任何密钥，机器人只列出来供参考（需要各自扫码 / 填密钥新建）。
import { Eye, Upload } from "lucide-react";
import { useState } from "react";
import { applyImport, errText, previewImport, type ImportPreview, type ImportSelect } from "../api";
import { ChipMultiSelect } from "./ui";

const SECTIONS: { key: keyof ImportSelect; label: string }[] = [
  { key: "projects", label: "项目" },
  { key: "dbs", label: "数据源" },
  { key: "templates", label: "流程模板" },
  { key: "skills", label: "技能设置" },
];

export default function ImportConfig({ onImported }: { onImported?: (select: ImportSelect) => void }) {
  const [path, setPath] = useState("");
  const [preview, setPreview] = useState<ImportPreview | null>(null);
  const [select, setSelect] = useState<ImportSelect>({ projects: [], dbs: [], templates: [], skills: [] });
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<string | null>(null);

  const doPreview = async () => {
    setError(null);
    setDone(null);
    try {
      const p = await previewImport(path.trim());
      setPreview(p);
      setSelect({ projects: p.projects, dbs: p.dbs, templates: p.templates, skills: p.skills });
    } catch (e) {
      setPreview(null);
      setError(errText(e));
    }
  };

  const doApply = async () => {
    setError(null);
    try {
      await applyImport(path.trim(), select);
      const n = SECTIONS.map((s) => (select[s.key].length ? `${select[s.key].length} 个${s.label}` : "")).filter(Boolean);
      setDone(`已导入：${n.join("、") || "无"}`);
      onImported?.(select);
    } catch (e) {
      setError(errText(e));
    }
  };


  return (
    <div className="grid" style={{ gap: 10 }} data-testid="import-config">
      <div className="flex gap-2">
        <input
          className="inp mono flex-1"
          aria-label="配置文件路径"
          placeholder="同事分享的 beetle-config.json 完整路径"
          value={path}
          onChange={(e) => {
            setPath(e.target.value);
            setPreview(null);
          }}
        />
        <button type="button" className="btn tr" disabled={!path.trim()} onClick={() => void doPreview()}>
          <Eye />
          预览
        </button>
      </div>
      {error && (
        <div className="alert-err" role="alert" style={{ fontSize: 12 }}>
          {error}
        </div>
      )}
      {preview && (
        <div className="card" style={{ padding: 12 }}>
          {SECTIONS.map((sec) =>
            preview[sec.key].length ? (
              <div key={sec.key} style={{ marginBottom: 8 }}>
                <span className="lbl">{sec.label}</span>
                <ChipMultiSelect
                  ariaLabel={sec.label}
                  mono
                  options={preview[sec.key]}
                  value={select[sec.key]}
                  onChange={(next) => setSelect((s) => ({ ...s, [sec.key]: next }))}
                />
              </div>
            ) : null,
          )}
          {preview.projects.length > 0 && (
            <div style={{ color: "var(--muted)", fontSize: 12, marginBottom: 8 }} data-testid="import-repo-hint">
              仓库地址里的账号 / 令牌没有导出，导入后如果仓库需要凭据，请在本机自行配置（Git 凭据管理器或 SSH 密钥）。
            </div>
          )}
          {preview.bots.length > 0 && (
            <div style={{ color: "var(--muted)", fontSize: 12, marginBottom: 8 }}>
              文件里还有 {preview.bots.length} 个机器人（{preview.bots.map((b) => b.name).join("、")}）：不含密钥，需要用「新增机器人」各自创建。
            </div>
          )}
          <div className="flex items-center justify-end gap-2">
            {done && <span style={{ color: "var(--success-fg)", fontSize: 12, marginRight: "auto" }}>{done}</span>}
            <button type="button" className="btn btn-pri btn-sm tr" onClick={() => void doApply()}>
              <Upload />
              导入选中项
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
