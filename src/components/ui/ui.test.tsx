import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { describe, expect, test, vi } from "vitest";
import { Checkbox, ChipMultiSelect, Modal, NumberInput, Segmented, Select, ToggleChip } from "./index";

const OPTS = [
  { value: "mysql", label: "mysql", description: "MySQL 只读查询" },
  { value: "mongo", label: "mongo" },
  { value: "pg", label: "postgres", disabled: true },
];

function SelectHarness({ onChange = () => {} }: { onChange?: (v: string) => void }) {
  const [v, setV] = useState("mysql");
  return (
    <>
      <Select
        ariaLabel="库类型"
        value={v}
        options={OPTS}
        onChange={(x) => {
          setV(x);
          onChange(x);
        }}
      />
      <button type="button">外面</button>
    </>
  );
}

describe("Select", () => {
  test("点开显示选项、当前项打勾，点选项后关闭并回调", async () => {
    const onChange = vi.fn();
    render(<SelectHarness onChange={onChange} />);
    const box = screen.getByRole("combobox", { name: "库类型" });
    expect(box).toHaveTextContent("mysql");
    expect(box).toHaveAttribute("aria-expanded", "false");
    await userEvent.click(box);
    expect(box).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("option", { name: /mysql/ })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("option", { name: /MySQL 只读查询/ })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("option", { name: "mongo" }));
    expect(onChange).toHaveBeenCalledWith("mongo");
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    expect(box).toHaveTextContent("mongo");
  });

  test("键盘：↓ 打开、↓ 移动并跳过禁用项、回车选中；Esc 关闭", async () => {
    const onChange = vi.fn();
    render(<SelectHarness onChange={onChange} />);
    const box = screen.getByRole("combobox", { name: "库类型" });
    box.focus();
    await userEvent.keyboard("{ArrowDown}");
    expect(screen.getByRole("listbox")).toBeInTheDocument();
    await userEvent.keyboard("{ArrowDown}");
    await userEvent.keyboard("{ArrowDown}"); // pg 禁用，绕回 mysql
    await userEvent.keyboard("{ArrowDown}");
    await userEvent.keyboard("{Enter}");
    expect(onChange).toHaveBeenCalledWith("mongo");
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();

    await userEvent.keyboard("{ArrowDown}");
    expect(screen.getByRole("listbox")).toBeInTheDocument();
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
  });

  test("点外部关闭，禁用项点了不生效", async () => {
    const onChange = vi.fn();
    render(<SelectHarness onChange={onChange} />);
    await userEvent.click(screen.getByRole("combobox", { name: "库类型" }));
    await userEvent.click(screen.getByRole("option", { name: "postgres" }));
    expect(onChange).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: "外面" }));
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
  });
});

describe("开关类控件", () => {
  test("ToggleChip 用 aria-pressed 表示开关状态", async () => {
    const onChange = vi.fn();
    const { rerender } = render(<ToggleChip on={false} onChange={onChange}>demo_config</ToggleChip>);
    const chip = screen.getByRole("button", { name: "demo_config" });
    expect(chip).toHaveAttribute("aria-pressed", "false");
    expect(chip).not.toHaveClass("on");
    await userEvent.click(chip);
    expect(onChange).toHaveBeenCalledWith(true);
    rerender(<ToggleChip on onChange={onChange}>demo_config</ToggleChip>);
    expect(chip).toHaveAttribute("aria-pressed", "true");
    expect(chip).toHaveClass("on");
  });

  test("ChipMultiSelect 新选的追加在后面，再点移除", async () => {
    function H() {
      const [v, setV] = useState<string[]>(["b"]);
      return (
        <>
          <ChipMultiSelect ariaLabel="库" options={["a", "b", "c"]} value={v} onChange={setV} />
          <span data-testid="v">{v.join(",")}</span>
        </>
      );
    }
    render(<H />);
    await userEvent.click(screen.getByRole("button", { name: "a" }));
    expect(screen.getByTestId("v")).toHaveTextContent("b,a");
    await userEvent.click(screen.getByRole("button", { name: "b" }));
    expect(screen.getByTestId("v")).toHaveTextContent("a");
  });

  test("Checkbox 是 role=checkbox，可点切换", async () => {
    const onChange = vi.fn();
    render(<Checkbox checked={false} onChange={onChange} label="Claude Code" description="已检测到 CLI" variant="card" />);
    const cb = screen.getByRole("checkbox", { name: /Claude Code/ });
    expect(cb).not.toBeChecked();
    await userEvent.click(cb);
    expect(onChange).toHaveBeenCalledWith(true);
  });

  test("Segmented：点击和左右方向键切换", async () => {
    const onChange = vi.fn();
    render(
      <Segmented
        ariaLabel="范围"
        value="7d"
        onChange={onChange}
        options={[
          { value: "today", label: "今天" },
          { value: "7d", label: "近 7 天" },
          { value: "30d", label: "近 30 天" },
        ]}
      />,
    );
    expect(screen.getByRole("radio", { name: "近 7 天" })).toHaveAttribute("aria-checked", "true");
    await userEvent.click(screen.getByRole("radio", { name: "今天" }));
    expect(onChange).toHaveBeenLastCalledWith("today");
    screen.getByRole("radio", { name: "近 7 天" }).focus();
    await userEvent.keyboard("{ArrowRight}");
    expect(onChange).toHaveBeenLastCalledWith("30d");
  });
});

describe("NumberInput", () => {
  test("−/+ 按步进改值并受 min / max 限制", async () => {
    function H() {
      const [v, setV] = useState("2");
      return <NumberInput ariaLabel="并发" value={v} min={1} max={3} onChange={setV} />;
    }
    render(<H />);
    const input = screen.getByLabelText("并发");
    await userEvent.click(screen.getByRole("button", { name: "并发加一" }));
    expect(input).toHaveValue(3);
    expect(screen.getByRole("button", { name: "并发加一" })).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: "并发减一" }));
    await userEvent.click(screen.getByRole("button", { name: "并发减一" }));
    expect(input).toHaveValue(1);
    expect(screen.getByRole("button", { name: "并发减一" })).toBeDisabled();
  });
});

describe("Modal", () => {
  test("Esc 和关闭按钮调用 onClose；closeDisabled 时不关", async () => {
    const onClose = vi.fn();
    const { rerender } = render(
      <Modal title="安装技能" onClose={onClose} footer={<button type="button">确认</button>}>
        <input aria-label="路径" />
      </Modal>,
    );
    const dialog = screen.getByRole("dialog", { name: "安装技能" });
    expect(dialog).toHaveAttribute("aria-modal", "true");
    // 打开时聚焦第一个输入框
    expect(screen.getByLabelText("路径")).toHaveFocus();
    await userEvent.keyboard("{Escape}");
    expect(onClose).toHaveBeenCalledTimes(1);
    await userEvent.click(screen.getByRole("button", { name: "关闭" }));
    expect(onClose).toHaveBeenCalledTimes(2);

    rerender(
      <Modal title="安装技能" onClose={onClose} closeDisabled>
        <input aria-label="路径" />
      </Modal>,
    );
    await userEvent.keyboard("{Escape}");
    expect(onClose).toHaveBeenCalledTimes(2);
  });

  test("Tab 焦点锁在弹窗内", async () => {
    render(
      <>
        <button type="button">外部按钮</button>
        <Modal title="弹窗" onClose={() => {}} footer={<button type="button">确认</button>}>
          <input aria-label="名称" />
        </Modal>
      </>,
    );
    expect(screen.getByLabelText("名称")).toHaveFocus();
    await userEvent.tab();
    expect(screen.getByRole("button", { name: "确认" })).toHaveFocus();
    await userEvent.tab();
    // 回到弹窗里的第一个可聚焦元素（标题栏的关闭按钮），不会跑到外部按钮
    expect(screen.getByRole("button", { name: "关闭" })).toHaveFocus();
  });
});
