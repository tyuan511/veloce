# AGENTS.md — Veloce 工程约定

## 最重要的一条：不要写 UI 测试

**不要为 GPUI 界面写测试。** 包括但不限于：

- `TestAppContext` / `VisualTestContext` / `#[gpui::test]`
- 任何需要 `add_window`、`cx.draw`、`simulate_keystrokes` 的测试
- 任何为了测试而给生产代码加的 `#[cfg(test)]` 钩子（`for_test`、`*_for_test`、
  `start_filter_quiet` 之类）
- 截图比对、像素断言

界面和交互由**用户本人**手动验证。Agent 的工作边界是：

- 把逻辑抽成**不依赖 `gpui::Context` / `Window` 的纯函数**，对这些纯函数写单元测试；
- 保证 `cargo build`、`cargo clippy`、`cargo test` 全绿；
- 改完界面后如实说明「UI 未做自动化验证，需人工确认」，不要声称已验证。

原因：GPUI 的测试上下文需要窗口、焦点栈和渲染帧，写出来的测试既脆弱又慢，
还逼着生产代码长出只为测试存在的分支。纯函数 + 手工验收的组合更诚实。

## 分层

```
crates/vfs-core/    无 GUI 依赖，纯逻辑，单元测试覆盖的主体
crates/veloce/      GPUI 前端；能抽成纯函数的逻辑都抽到模块顶层函数里
```

判断标准：一个函数如果只吃普通数据结构（`&[FileEntry]`、`&str`、`usize`），
就应该放在 `vfs-core` 或作为 `veloce` 里的自由函数，并配单元测试。
如果它需要 `cx`、`window`、`Entity`，那它属于渲染/事件层，不写测试。

## 测试要求

- 每个新增的纯逻辑分支都要有测试：正常路径 + 边界（空输入、越界、单元素）+ 错误路径。
- 测试名描述**行为**，不是描述函数名：`clamps_cursor_at_both_ends` 而不是 `test_clamp`。
- 用 `tempfile` 建真实临时目录，不要 mock 文件系统。
- 不许 `#[ignore]`、不许 sleep 等待、不许依赖测试执行顺序。
- 提交前必须 `cargo test` 全绿，`cargo build` 零 warning。

## 代码风格

- 注释解释**为什么**，不解释是什么。中文注释可以，代码标识符用英文。
- 错误要带上下文（路径、操作名），不要 `unwrap()` 到用户脸上；UI 层可以把错误
  渲染成可读文案，但核心层必须返回结构化错误。
- 不要在 UI 线程做文件 I/O 或递归遍历；耗时操作一律进 `TaskManager`。
- 删除默认进系统回收站（`trash` crate），不提供硬删除路径。

## 提交

**Commit message 一律用英文。** 不论对话用哪种语言，仓库里的提交信息都是英文，
这样 GitHub 上的提交历史和 `git log` 对所有贡献者可读。

- 格式：conventional commits，`<type>: <imperative summary>`
  （`feat:` / `fix:` / `refactor:` / `test:` / `docs:` / `chore:`）。
- 标题行不超过 72 字符，祈使句、首字母小写、句尾不加句号：
  `feat: add in-memory cross-protocol transfer pipe`，不是
  `feat: 添加跨协议传输` 也不是 `Added transfer pipe.`
- 正文用英文，说明**为什么**而不是罗列改了什么；按需分点或分段，每行 80 字符内。
- 不要写 `Co-Authored-By` 之类未要求的 trailer，除非用户明确要求。
- 不要顺手提交 `target/`、`.DS_Store`、临时脚本。
- 作者身份用仓库配置的 `tyuan511 <tangge0601@gmail.com>`，不要临时指定别的身份。

反例（会被打回）：中文提交信息、`update` / `fix bug` 这类无信息量的标题、
把 diff 逐行翻译成正文。
