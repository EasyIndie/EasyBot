//! 构建脚本：自动配置 Git hooks
//!
//! 在 cargo build 时自动设置 core.hooksPath = .githooks，
//! 新贡献者克隆后无需手动运行 `make setup`。

fn main() {
    // 运行 git config core.hooksPath .githooks
    // 将 hooks 目录指向 repo 中的 .githooks/，使 git commit/push 自动执行其中的 hook 脚本
    std::process::Command::new("git")
        .args(["config", "core.hooksPath", ".githooks"])
        .status()
        .ok();
}
